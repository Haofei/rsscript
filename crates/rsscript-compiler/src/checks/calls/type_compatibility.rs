//! Shared structural type compatibility for call arguments and collection literals.

use super::*;

pub(crate) use rsscript_semantics::type_compatible as argument_type_matches;
pub(super) use rsscript_semantics::type_contains_unresolved_generic;

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
    if has_unresolved_generic_fact(analyzer, expected) {
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
    if has_unresolved_generic_fact(analyzer, actual) || argument_type_matches(expected, actual) {
        return;
    }
    analyzer.diagnostics.push(
        rsscript_semantics::map_literal_entry_type_mismatch_diagnostic(
            role,
            actual,
            expected,
            context,
            hir_expr_span(value).clone(),
        ),
    );
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
    if has_unresolved_generic_fact(analyzer, expected) {
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
    if has_unresolved_generic_fact(analyzer, actual) || argument_type_matches(expected, actual) {
        return;
    }
    analyzer.diagnostics.push(
        rsscript_semantics::list_literal_item_type_mismatch_diagnostic(
            actual,
            expected,
            context,
            hir_expr_span(value).clone(),
        ),
    );
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

pub(crate) fn has_unresolved_generic_fact(analyzer: &Analyzer<'_>, type_name: &str) -> bool {
    let mut facts = rsscript_semantics::UnresolvedGenericFacts::default();
    facts.declared_type_names.extend(
        analyzer
            .syntax_program
            .items
            .iter()
            .chain(
                analyzer
                    .interface_programs
                    .iter()
                    .flat_map(|program| program.items.iter()),
            )
            .filter_map(|item| match item {
                Item::Type(decl) => Some(decl.name.clone()),
                Item::SumType(decl) => Some(decl.name.clone()),
                Item::TypeAlias(decl) => Some(decl.name.clone()),
                _ => None,
            }),
    );
    rsscript_semantics::contains_unresolved_generic_type(type_name, &facts)
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
