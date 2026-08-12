//! Semantic generic-bound diagnostics independent of compiler orchestration.

use std::collections::HashMap;

use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{GenericBound, GenericParam, Item, Program, TypeKind, TypeRef};

/// The source-independent facts needed to decide whether a concrete type
/// satisfies a protocol bound at a resolved call site.
#[derive(Debug, Clone, Default)]
pub struct ProtocolSatisfactionFacts {
    pub caller_type_param_bounds: HashMap<String, Option<GenericBound>>,
    pub visible_protocol_impls: Vec<(String, String)>,
    pub declared_derives: HashMap<String, Vec<String>>,
}

/// Decide a resolved protocol-bound fact without compiler HIR or runtime
/// dependencies. The compiler supplies the visible implementation inventory;
/// semantics owns the language rule for builtin and structural containers.
pub fn type_satisfies_protocol_bound(
    actual: &str,
    protocol: &str,
    facts: &ProtocolSatisfactionFacts,
) -> bool {
    let actual_root = crate::type_root_name(strip_fresh_type(actual));
    if dyn_protocol(actual).is_some_and(|dyn_protocol| dyn_protocol == protocol) {
        return true;
    }
    if protocol == "Ord" && builtin_type_is_ord(actual_root) {
        return true;
    }
    if (protocol == "Hashable" || protocol == "Eq") && builtin_type_is_hashable(actual_root) {
        return true;
    }
    if protocol == "Clone" && builtin_type_is_clone(actual_root) {
        return true;
    }
    if (protocol == "Hashable" || protocol == "Eq" || protocol == "Clone")
        && matches!(actual_root, "List" | "Option" | "Result")
        && let Some(args) = crate::type_arg_names(strip_fresh_type(actual))
    {
        return args
            .iter()
            .all(|arg| type_satisfies_protocol_bound(arg, protocol, facts));
    }
    if facts
        .caller_type_param_bounds
        .get(actual_root)
        .and_then(Option::as_ref)
        .is_some_and(|bound| matches!(bound, GenericBound::Protocol(bound) if bound == protocol))
    {
        return true;
    }
    if facts
        .visible_protocol_impls
        .iter()
        .any(|(implemented_protocol, type_name)| {
            implemented_protocol == protocol && type_name == actual_root
        })
    {
        return true;
    }
    facts
        .declared_derives
        .get(actual_root)
        .is_some_and(|derives| type_derives_protocol(derives, protocol))
}

/// Extract a platform-neutral protocol-satisfaction inventory from syntax.
pub fn protocol_satisfaction_facts<'a>(
    function_type_params: &[GenericParam],
    visible_protocol_impls: impl IntoIterator<Item = (String, String)>,
    programs: impl IntoIterator<Item = &'a Program>,
) -> ProtocolSatisfactionFacts {
    let mut facts = ProtocolSatisfactionFacts {
        caller_type_param_bounds: function_type_params
            .iter()
            .map(|parameter| (parameter.name.clone(), parameter.bound.clone()))
            .collect(),
        visible_protocol_impls: visible_protocol_impls.into_iter().collect(),
        declared_derives: HashMap::new(),
    };
    for program in programs {
        for item in &program.items {
            match item {
                Item::Type(decl) => {
                    facts
                        .declared_derives
                        .insert(decl.name.clone(), decl.derives.clone());
                }
                Item::SumType(decl) => {
                    facts
                        .declared_derives
                        .insert(decl.name.clone(), decl.derives.clone());
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::TypeAlias(_)
                | Item::Const(_)
                | Item::Function(_) => {}
            }
        }
    }
    facts
}

/// Return the semantic guidance paired with a failed generic protocol bound.
pub fn protocol_bound_guidance(protocol: &str, actual: &str) -> (&'static str, String) {
    match protocol {
        "Hashable" => (
            "A `Map` key / `Set` element must be `Hashable` (and therefore `Eq`). Hashability is a compiler-derived structural contract: a builtin scalar key, or a managed struct/sum that derives `Eq` and `Hash`.",
            format!(
                "Add `derives(Eq, Hash)` to `{actual}` so the compiler derives a structural hash and equality, or use a hashable key type."
            ),
        ),
        "Eq" => (
            "Equality is a compiler-derived structural contract: a builtin scalar, or a managed struct/sum that derives `Eq` (or `Ord`, which implies `Eq`).",
            format!("Add `derives(Eq)` to `{actual}`, or use an equatable type."),
        ),
        _ => (
            "Generic protocol bounds are nominal. Use a type with a matching derive, add a compatible generic bound, or pass an explicit comparator API.",
            format!(
                "Add `derives({protocol})` to `{actual}` if the compiler-owned ordering is intended, or call an API that accepts an explicit comparator."
            ),
        ),
    }
}

/// Whether a rendered resolved type is a dynamic protocol value.
pub fn dynamic_protocol_name(type_name: &str) -> Option<&str> {
    dyn_protocol(type_name)
}

fn strip_fresh_type(type_name: &str) -> &str {
    type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name.trim())
}

fn dyn_protocol(type_name: &str) -> Option<&str> {
    (crate::type_root_name(strip_fresh_type(type_name)) == "Dyn")
        .then(|| crate::type_arg_names(type_name))
        .flatten()
        .and_then(|args| args.first().copied())
}

fn builtin_type_is_ord(type_name: &str) -> bool {
    matches!(type_name, "Int" | "String" | "Bool")
}

fn builtin_type_is_hashable(type_name: &str) -> bool {
    matches!(
        type_name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Bool"
            | "Byte"
            | "Char"
            | "Unit"
            | "String"
    )
}

fn builtin_type_is_clone(type_name: &str) -> bool {
    matches!(
        type_name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Bool"
            | "Byte"
            | "Char"
            | "Unit"
            | "Float"
            | "Float32"
            | "Float64"
            | "String"
    )
}

fn type_derives_protocol(derives: &[String], protocol: &str) -> bool {
    let has = |name: &str| derives.iter().any(|derive| derive == name);
    match protocol {
        "Ord" => has("Ord"),
        "Hashable" => has("Hash"),
        "Eq" => has("Eq") || has("Ord"),
        "Clone" => has("Clone"),
        _ => false,
    }
}

/// Validate resource generic fields and fresh generic return requirements.
pub fn generic_constraint_diagnostics(program: &Program) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) if decl.kind == TypeKind::Resource => {
                let bounds = generic_bounds(&decl.type_params);
                for param in &decl.type_params {
                    if param.bound.is_none() {
                        diagnostics.push(generic_resource_argument_diagnostic(
                            &param.name,
                            &param.name,
                            &param.span,
                            "resource type parameters must declare an explicit bound.",
                        ));
                    }
                }
                for field in &decl.fields {
                    collect_resource_type_param_field(&field.ty, &bounds, &mut diagnostics);
                }
            }
            Item::Function(function) if function.returns_fresh => {
                let Some(return_ty) = &function.return_ty else {
                    continue;
                };
                let bounds = generic_bounds(&function.type_params);
                let target = fresh_return_target_type(return_ty);
                let bound = bounds.get(&target.name).and_then(Option::as_ref);
                let fresh_bound_ok = matches!(bound, Some(GenericBound::Struct))
                    || (target.name == "Self" && matches!(bound, Some(GenericBound::Managed)));
                if bounds.contains_key(&target.name) && !fresh_bound_ok {
                    diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_FRESH_RETURN_TYPE,
                            format!(
                                "function `{}` returns `fresh {}` but `{}` is not bounded by `Struct`.",
                                function.name, target.name, target.name
                            ),
                            target.span.clone(),
                            "invalid fresh generic type",
                        )
                        .with_cause("A generic `fresh T` return must require `T: Struct` so freshness is valid for every instantiation.")
                        .with_fix(
                            "add_struct_bound",
                            format!("Declare `{}` with `{}: Struct`, or remove `fresh`.", target.name, target.name),
                            "manual",
                        ),
                    );
                }
            }
            _ => {}
        }
    }
    diagnostics
}

/// Construct the canonical diagnostic for a resolved generic protocol-bound
/// failure. Resolution and structural protocol satisfaction remain frontend
/// facts; the language-facing contract belongs to semantics.
pub fn protocol_bound_not_satisfied_diagnostic(
    actual: &str,
    protocol: &str,
    call_name: &str,
    span: rsscript_syntax::Span,
    cause: &'static str,
    fix: String,
) -> Diagnostic {
    Diagnostic::error(
        code::PROTOCOL_NOT_SATISFIED,
        format!(
            "type `{actual}` does not satisfy protocol `{protocol}` required by `{call_name}`."
        ),
        span,
        "protocol not satisfied",
    )
    .with_cause(cause)
    .with_fix("satisfy_protocol_bound", fix, "manual")
}

/// Construct the canonical diagnostic for an invalid `Dyn<Protocol>`
/// construction after the compiler has resolved the wrapped value type.
pub fn dyn_from_diagnostic(
    protocol: &str,
    value_type: &str,
    span: rsscript_syntax::Span,
    cause: &'static str,
) -> Diagnostic {
    Diagnostic::error(
        code::PROTOCOL_NOT_SATISFIED,
        format!("cannot construct `Dyn<{protocol}>` from `{value_type}`."),
        span,
        "external_binding protocol not satisfied",
    )
    .with_cause(cause)
    .with_cause(
        "Dyn values are explicit dynamic protocol boundaries; construction requires a concrete value with a visible protocol implementation.",
    )
    .with_fix(
        "add_protocol_impl",
        format!("Declare `impl {protocol} for {value_type} {{ ... }}` or wrap a value that already satisfies `{protocol}`."),
        "manual",
    )
}

/// Construct the canonical diagnostic for a positional user-variant field.
pub fn unnamed_variant_field_diagnostic(variant: &str, span: rsscript_syntax::Span) -> Diagnostic {
    Diagnostic::error(
        code::UNNAMED_ARGUMENT,
        format!(
            "variant `{variant}` must be constructed with named fields, e.g. `{variant}(field: value)`."
        ),
        span,
        "variant field must be named",
    )
}

/// Construct the canonical diagnostic for an unknown user-variant field.
pub fn unknown_variant_field_diagnostic(
    variant: &str,
    field: &str,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_ARGUMENT,
        format!("variant `{variant}` has no field `{field}`."),
        span,
        "unknown variant field",
    )
}

/// Construct the canonical diagnostic for excess user-variant fields.
pub fn too_many_variant_fields_diagnostic(
    variant: &str,
    expected: usize,
    actual: usize,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_ARGUMENT,
        format!("variant `{variant}` has {expected} field(s) but {actual} were given."),
        span,
        "too many variant fields",
    )
}

/// Construct the canonical diagnostic for a repeated user-variant field.
pub fn duplicate_variant_field_diagnostic(
    variant: &str,
    field: &str,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::DUPLICATE_ARGUMENT,
        format!("variant `{variant}` field `{field}` is provided more than once."),
        span,
        "duplicate variant field",
    )
}

/// Construct the canonical diagnostic for a user-variant field type mismatch.
pub fn variant_field_type_mismatch_diagnostic(
    variant: &str,
    field: &str,
    actual: &str,
    expected: &str,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("variant `{variant}` field `{field}` has type `{actual}`, expected `{expected}`."),
        span,
        "variant field type mismatch",
    )
}

/// Construct the canonical diagnostic for a missing user-variant field.
pub fn missing_variant_field_diagnostic(
    variant: &str,
    field: &str,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::MISSING_ARGUMENT,
        format!("variant `{variant}` is missing field `{field}`."),
        span,
        "missing variant field",
    )
}

/// Construct the canonical diagnostic for a malformed standard sum variant.
pub fn conventional_variant_form_diagnostic(
    variant: &str,
    form: &str,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::UNSUPPORTED_SYNTAX,
        format!("variant `{variant}` must use its conventional RSScript form."),
        span,
        "unsupported variant form",
    )
    .with_cause("Standard Result and Option variants are call-like for checker purposes, but they are not ordinary named-argument calls.")
    .with_fix(
        "use_conventional_variant_form",
        format!("Write this variant as {form}."),
        "manual",
    )
}

/// Construct the canonical diagnostic for a resolved protocol receiver that
/// has no matching implementation or generic bound.
pub fn protocol_receiver_not_satisfied_diagnostic(
    receiver_type: &str,
    receiver_root: &str,
    protocol: &str,
    method: &str,
    span: rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::PROTOCOL_NOT_SATISFIED,
        format!(
            "receiver type `{receiver_type}` does not satisfy protocol `{protocol}` for `{protocol}.{method}`."
        ),
        span,
        "protocol not satisfied",
    )
    .with_cause("Protocols are nominal external_binding contracts. A protocol call must be backed by an explicit generic bound or an explicit protocol implementation.")
    .with_fix(
        "add_protocol_bound_or_impl",
        format!("Add a `{receiver_root}: {protocol}` generic bound or declare `impl {protocol} for {receiver_root} {{ ... }}`."),
        "manual",
    )
}

fn generic_bounds(params: &[GenericParam]) -> HashMap<String, Option<GenericBound>> {
    params
        .iter()
        .map(|param| (param.name.clone(), param.bound.clone()))
        .collect()
}

fn collect_resource_type_param_field(
    ty: &TypeRef,
    bounds: &HashMap<String, Option<GenericBound>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource) {
        diagnostics.push(generic_resource_argument_diagnostic(
            &ty.name,
            &ty.name,
            &ty.span,
            "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
        ));
    }
    for arg in &ty.args {
        collect_resource_type_param_field(arg, bounds, diagnostics);
    }
}

fn fresh_return_target_type(return_ty: &TypeRef) -> &TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}

fn generic_resource_argument_diagnostic(
    generic_name: &str,
    resource_name: &str,
    span: &rsscript_syntax::Span,
    cause: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_GENERIC_ARGUMENT,
        format!("generic type `{generic_name}` cannot be used with resource `{resource_name}`."),
        span.clone(),
        "resource generic misuse",
    )
    .with_cause(cause)
    .with_fix(
        "add_or_change_resource_bound",
        "Do not store `T: Resource` in a generic value.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_resource_fields_and_fresh_generic_returns() {
        let program = rsscript_syntax::parse_source(
            "constraints.rss",
            r#"
resource Bag<T: Resource> { value: T }
fn make<T: Managed>() -> fresh T
"#,
        );
        let diagnostics = generic_constraint_diagnostics(&program);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, code::RESOURCE_GENERIC_ARGUMENT);
        assert_eq!(diagnostics[1].code, code::INVALID_FRESH_RETURN_TYPE);
    }

    #[test]
    fn derives_resolved_protocol_and_variant_diagnostics() {
        let span = rsscript_syntax::Span {
            file: "generic.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let diagnostics = [
            protocol_bound_not_satisfied_diagnostic(
                "Widget",
                "Clone",
                "copy",
                span.clone(),
                "a protocol bound failed",
                "Use a cloneable value.".into(),
            ),
            dyn_from_diagnostic("Display", "Widget", span.clone(), "missing impl"),
            unnamed_variant_field_diagnostic("Event", span.clone()),
            unknown_variant_field_diagnostic("Event", "bogus", span.clone()),
            too_many_variant_fields_diagnostic("Event", 1, 2, span.clone()),
            duplicate_variant_field_diagnostic("Event", "name", span.clone()),
            variant_field_type_mismatch_diagnostic("Event", "name", "Int", "String", span.clone()),
            missing_variant_field_diagnostic("Event", "name", span.clone()),
            conventional_variant_form_diagnostic("None", "`None`", span.clone()),
            protocol_receiver_not_satisfied_diagnostic("Widget", "Widget", "Display", "show", span),
        ];

        assert_eq!(diagnostics[0].code, code::PROTOCOL_NOT_SATISFIED);
        assert_eq!(diagnostics[1].code, code::PROTOCOL_NOT_SATISFIED);
        assert_eq!(diagnostics[2].code, code::UNNAMED_ARGUMENT);
        assert_eq!(diagnostics[3].code, code::UNKNOWN_ARGUMENT);
        assert_eq!(diagnostics[4].code, code::UNKNOWN_ARGUMENT);
        assert_eq!(diagnostics[5].code, code::DUPLICATE_ARGUMENT);
        assert_eq!(diagnostics[6].code, code::ARGUMENT_TYPE_MISMATCH);
        assert_eq!(diagnostics[7].code, code::MISSING_ARGUMENT);
        assert_eq!(diagnostics[8].code, code::UNSUPPORTED_SYNTAX);
        assert_eq!(diagnostics[9].code, code::PROTOCOL_NOT_SATISFIED);
    }

    #[test]
    fn evaluates_protocol_satisfaction_from_neutral_facts() {
        let facts = ProtocolSatisfactionFacts {
            declared_derives: HashMap::from([(
                "Coordinate".to_owned(),
                vec!["Eq".to_owned(), "Hash".to_owned()],
            )]),
            ..ProtocolSatisfactionFacts::default()
        };
        assert!(type_satisfies_protocol_bound(
            "List<Coordinate>",
            "Hashable",
            &facts
        ));
        assert!(type_satisfies_protocol_bound("Int", "Clone", &facts));
        assert!(!type_satisfies_protocol_bound("Float", "Eq", &facts));
    }
}
