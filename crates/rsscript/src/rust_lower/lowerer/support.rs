//! Shared lowering predicates, formatting helpers, and structural substitutions.

use super::*;

/// Whether any arm matches with a list slice pattern, so the scrutinee must be
/// lowered as a `[T]` slice rather than a `Vec<T>`.
pub(in crate::rust_lower) fn arms_have_list_pattern(arms: &[crate::syntax::ast::MatchArm]) -> bool {
    arms.iter()
        .any(|arm| matches!(arm.pattern, MatchPattern::List { .. }))
}

pub(in crate::rust_lower) fn match_pattern_span(pattern: &MatchPattern) -> Span {
    match pattern {
        MatchPattern::Variant { span, .. }
        | MatchPattern::Struct { span, .. }
        | MatchPattern::Literal { span, .. }
        | MatchPattern::List { span, .. }
        | MatchPattern::Binding { span, .. }
        | MatchPattern::Wildcard(span) => span.clone(),
    }
}

pub(in crate::rust_lower) fn capability_enum_name(protocol: &str) -> String {
    format!("Capability{}", rust_ident(protocol))
}

pub(in crate::rust_lower) fn capability_impl_forward_arg(param: &Param) -> String {
    if param.name == "self" {
        "inner".to_string()
    } else {
        rust_value_ident(&param.name)
    }
}

pub(in crate::rust_lower) fn capability_from_protocol(callee: &Callee) -> Option<&str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    if type_root_name(namespace) != "Capability" || type_root_name(name) != "from" {
        return None;
    }
    type_arg_names(namespace).and_then(|args| args.first().copied())
}

pub(in crate::rust_lower) fn capability_protocol_name(type_name: &str) -> Option<&str> {
    if type_root_name(type_name) != "Capability" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().copied())
}

pub(in crate::rust_lower) fn type_ref_display_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }
    format!(
        "{}<{}>",
        ty.name,
        ty.args
            .iter()
            .map(type_ref_display_name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(in crate::rust_lower) fn closure_binding(name: &str, is_mut: bool) -> String {
    let name = rust_ident(name);
    if is_mut { format!("mut {name}") } else { name }
}

pub(in crate::rust_lower) fn awaited_binding_type(
    value_type: Option<TypeRef>,
    is_try: bool,
) -> Option<TypeRef> {
    let value_type = value_type?;
    if is_try {
        result_ok_type_ref(&value_type)
    } else {
        Some(value_type)
    }
}

pub(in crate::rust_lower) fn stmt_contains_await(statement: &Stmt) -> bool {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().is_some_and(expr_contains_await),
        Stmt::Return(stmt) => stmt.value.as_ref().is_some_and(expr_contains_await),
        Stmt::With(stmt) => expr_contains_await(&stmt.resource) || block_contains_await(&stmt.body),
        Stmt::If(stmt) => {
            expr_contains_await(&stmt.condition)
                || block_contains_await(&stmt.then_body)
                || stmt.else_body.as_ref().is_some_and(block_contains_await)
        }
        Stmt::Loop(stmt) => {
            stmt.condition.as_ref().is_some_and(expr_contains_await)
                || block_contains_await(&stmt.body)
        }
        Stmt::For(stmt) => {
            stmt.is_async || expr_contains_await(&stmt.iterable) || block_contains_await(&stmt.body)
        }
        Stmt::TaskGroup(_) | Stmt::Select(_) => true,
        Stmt::Match(stmt) => {
            expr_contains_await(&stmt.value)
                || stmt.arms.iter().any(|arm| block_contains_await(&arm.body))
        }
        Stmt::LetElse(stmt) => {
            expr_contains_await(&stmt.value) || block_contains_await(&stmt.else_body)
        }
        Stmt::Assign(stmt) => expr_contains_await(&stmt.target) || expr_contains_await(&stmt.value),
        Stmt::Expr(expr) => expr_contains_await(expr),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => false,
    }
}

pub(in crate::rust_lower) fn block_contains_await(block: &Block) -> bool {
    block.statements.iter().any(stmt_contains_await)
}

pub(in crate::rust_lower) fn stmt_contains_try(statement: &Stmt) -> bool {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().is_some_and(expr_contains_try),
        Stmt::Return(stmt) => stmt.value.as_ref().is_some_and(expr_contains_try),
        Stmt::With(stmt) => expr_contains_try(&stmt.resource) || block_contains_try(&stmt.body),
        Stmt::If(stmt) => {
            expr_contains_try(&stmt.condition)
                || block_contains_try(&stmt.then_body)
                || stmt.else_body.as_ref().is_some_and(block_contains_try)
        }
        Stmt::Loop(stmt) => {
            stmt.condition.as_ref().is_some_and(expr_contains_try) || block_contains_try(&stmt.body)
        }
        Stmt::For(stmt) => expr_contains_try(&stmt.iterable) || block_contains_try(&stmt.body),
        Stmt::Match(stmt) => {
            expr_contains_try(&stmt.value)
                || stmt.arms.iter().any(|arm| block_contains_try(&arm.body))
        }
        Stmt::LetElse(stmt) => {
            expr_contains_try(&stmt.value) || block_contains_try(&stmt.else_body)
        }
        Stmt::Assign(stmt) => expr_contains_try(&stmt.target) || expr_contains_try(&stmt.value),
        Stmt::Expr(expr) => expr_contains_try(expr),
        Stmt::TaskGroup(_)
        | Stmt::Select(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => false,
    }
}

pub(in crate::rust_lower) fn block_contains_try(block: &Block) -> bool {
    block.statements.iter().any(stmt_contains_try)
}

/// Substitute concrete `args` for the generic type parameters `params` in `ty`.
/// A declared field type that names a parameter (e.g. `A` in `__Tuple2<A, B>`)
/// resolves to the matching argument from the scrutinee's `value_type`; nested
/// type arguments are substituted recursively. Unresolved names are left as-is
/// (treated as non-`Copy`, cloneable values).
pub(in crate::rust_lower) fn substitute_generic_type(
    ty: &TypeRef,
    params: &[String],
    args: &[TypeRef],
) -> TypeRef {
    if ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && let Some(index) = params.iter().position(|param| param == &ty.name)
        && let Some(arg) = args.get(index)
    {
        return arg.clone();
    }
    let mut resolved = ty.clone();
    resolved.args = ty
        .args
        .iter()
        .map(|arg| substitute_generic_type(arg, params, args))
        .collect();
    resolved.fn_params = ty
        .fn_params
        .iter()
        .map(|param| substitute_generic_type(param, params, args))
        .collect();
    resolved.fn_return = ty
        .fn_return
        .as_ref()
        .map(|ret| Box::new(substitute_generic_type(ret, params, args)));
    resolved
}

pub(in crate::rust_lower) fn match_binding_type_ref(
    pattern: &MatchPattern,
    value_type: Option<&TypeRef>,
) -> Option<(String, TypeRef)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.cloned().map(|ty| (name.clone(), ty));
    }
    let MatchPattern::Variant { name, bindings, .. } = pattern else {
        return None;
    };
    // Option/Result carry a single positional payload.
    let Some(binding) = bindings.first() else {
        return None;
    };
    let value_type = value_type?;
    match name.as_str() {
        "Some" if value_type.name == "Option" => value_type
            .args
            .first()
            .cloned()
            .and_then(|ty| match_binding_type_ref(binding, Some(&ty))),
        "Ok" if value_type.name == "Result" => value_type
            .args
            .first()
            .cloned()
            .and_then(|ty| match_binding_type_ref(binding, Some(&ty))),
        "Err" if value_type.name == "Result" => value_type
            .args
            .get(1)
            .cloned()
            .and_then(|ty| match_binding_type_ref(binding, Some(&ty))),
        _ => None,
    }
}

pub(in crate::rust_lower) fn split_loop_body_at_first_await(
    statements: &[Stmt],
) -> Option<(&[Stmt], &Stmt, &[Stmt])> {
    for (index, statement) in statements.iter().enumerate() {
        match statement {
            Stmt::Let(stmt) if stmt.value.as_ref().and_then(async_await_inner).is_some() => {
                return Some((&statements[..index], statement, &statements[index + 1..]));
            }
            Stmt::Expr(expr) if async_await_inner(expr).is_some() => {
                return Some((&statements[..index], statement, &statements[index + 1..]));
            }
            other if stmt_contains_await(other) => return None,
            _ => {}
        }
    }
    None
}

pub(in crate::rust_lower) fn expr_contains_await(expr: &Expr) -> bool {
    match expr {
        Expr::Await { .. } => true,
        Expr::Try { value, .. }
        | Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. } => expr_contains_await(value),
        Expr::Binary { left, right, .. } => expr_contains_await(left) || expr_contains_await(right),
        Expr::Call { args, .. } => args.iter().any(|arg| expr_contains_await(&arg.value)),
        Expr::Field { base, .. } => expr_contains_await(base),
        Expr::Index { base, index, .. } => expr_contains_await(base) || expr_contains_await(index),
        Expr::Closure { body, .. } => block_contains_await(body),
        Expr::Match { value, arms, .. } => {
            expr_contains_await(value) || arms.iter().any(|arm| block_contains_await(&arm.body))
        }
        Expr::ObjectLiteral { fields, .. } => {
            fields.iter().any(|field| expr_contains_await(&field.value))
        }
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .any(|entry| expr_contains_await(&entry.key) || expr_contains_await(&entry.value)),
        Expr::ArrayLiteral { items, .. } => items.iter().any(expr_contains_await),
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => false,
    }
}

pub(in crate::rust_lower) fn expr_contains_try(expr: &Expr) -> bool {
    match expr {
        Expr::Try { .. } => true,
        Expr::Await { value, .. }
        | Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. } => expr_contains_try(value),
        Expr::Binary { left, right, .. } => expr_contains_try(left) || expr_contains_try(right),
        Expr::Call { args, .. } => args.iter().any(|arg| expr_contains_try(&arg.value)),
        Expr::Field { base, .. } => expr_contains_try(base),
        Expr::Index { base, index, .. } => expr_contains_try(base) || expr_contains_try(index),
        Expr::Closure { body, .. } => block_contains_try(body),
        Expr::Match { value, arms, .. } => {
            expr_contains_try(value) || arms.iter().any(|arm| block_contains_try(&arm.body))
        }
        Expr::ObjectLiteral { fields, .. } => {
            fields.iter().any(|field| expr_contains_try(&field.value))
        }
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .any(|entry| expr_contains_try(&entry.key) || expr_contains_try(&entry.value)),
        Expr::ArrayLiteral { items, .. } => items.iter().any(expr_contains_try),
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => false,
    }
}

/// Join already-lowered operands with a short-circuit operator (`&&`/`||`) as a
/// *balanced* tree, so the resulting Rust expression nests O(log n) deep rather
/// than O(n). `&&`/`||` are associative for both value and evaluation order, so
/// any grouping evaluates the operands left-to-right with identical short-circuit
/// behavior. Recursion depth here is O(log n), so this helper is itself safe.
pub(in crate::rust_lower) fn balanced_logical_join(leaves: &[String], op: &str) -> String {
    match leaves.len() {
        0 => String::new(),
        1 => leaves[0].clone(),
        n => {
            let mid = n / 2;
            format!(
                "({} {op} {})",
                balanced_logical_join(&leaves[..mid], op),
                balanced_logical_join(&leaves[mid..], op)
            )
        }
    }
}

/// Binding tightness of a `BinaryOp` in the *Rust* grammar (higher binds
/// tighter). `lower_binary_operand` uses this to decide when a child operand
/// needs parentheses to preserve the RSScript AST's grouping in the emitted
/// Rust. It MUST mirror Rust's precedence, not C's: Rust binds the bitwise
/// operators (`&`, `^`, `|`) TIGHTER than the comparison operators, so
/// `a & (b == c)` must be parenthesized (unparenthesized, `a & b == c` regroups
/// to `(a & b) == c` in Rust and mis-lowers).
pub(in crate::rust_lower) fn rust_binary_precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::LogicalOr => 1,
        BinaryOp::LogicalAnd => 2,
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual => 3,
        BinaryOp::BitOr => 4,
        BinaryOp::BitXor => 5,
        BinaryOp::BitAnd => 6,
        BinaryOp::ShiftLeft | BinaryOp::ShiftRight => 7,
        BinaryOp::Add | BinaryOp::Subtract => 8,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 9,
    }
}

pub(in crate::rust_lower) fn rust_binary_is_comparison(op: BinaryOp) -> bool {
    matches!(
        op,
        BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
    )
}

pub(in crate::rust_lower) fn type_ref_from_display(name: &str, span: &Span) -> TypeRef {
    let mut name = name.trim();
    let mut is_fresh = false;
    let mut is_noescape = false;
    let mut is_owned = false;
    loop {
        if let Some(rest) = name.strip_prefix("fresh ") {
            is_fresh = true;
            name = rest.trim();
        } else if let Some(rest) = name.strip_prefix("noescape ") {
            is_noescape = true;
            name = rest.trim();
        } else if let Some(rest) = name.strip_prefix("owned ") {
            is_owned = true;
            name = rest.trim();
        } else {
            break;
        }
    }

    if let Some(params_start) = name.strip_prefix("Fn(") {
        let mut depth = 0usize;
        let close = params_start
            .char_indices()
            .find_map(|(index, ch)| match ch {
                '(' => {
                    depth += 1;
                    None
                }
                ')' if depth == 0 => Some(index),
                ')' => {
                    depth -= 1;
                    None
                }
                _ => None,
            });
        if let Some(close) = close {
            let params_text = &params_start[..close];
            let mut fn_params = Vec::new();
            let mut fn_param_effects = Vec::new();
            for param in crate::text_util::split_top_level_type_args(params_text) {
                let (effect, param) = if let Some(param) = param.strip_prefix("read ") {
                    (Some(DataEffect::Read), param)
                } else if let Some(param) = param.strip_prefix("mut ") {
                    (Some(DataEffect::Mut), param)
                } else if let Some(param) = param.strip_prefix("take ") {
                    (Some(DataEffect::Take), param)
                } else {
                    (None, param)
                };
                if !param.is_empty() {
                    fn_params.push(type_ref_from_display(param, span));
                    fn_param_effects.push(effect);
                }
            }
            let rest = params_start[close + 1..].trim();
            let fn_return = rest
                .strip_prefix("->")
                .map(str::trim)
                .filter(|return_type| !return_type.is_empty())
                .map(|return_type| Box::new(type_ref_from_display(return_type, span)));
            return TypeRef {
                name: "Fn".to_string(),
                args: Vec::new(),
                malformed_arg_spans: Vec::new(),
                is_fresh,
                is_noescape,
                is_owned,
                fn_params,
                fn_param_effects,
                fn_return,
                span: span.clone(),
            };
        }
    }

    TypeRef {
        name: type_root_name(name).to_string(),
        args: type_arg_names(name)
            .unwrap_or_default()
            .into_iter()
            .map(|arg| type_ref_from_display(arg, span))
            .collect(),
        malformed_arg_spans: Vec::new(),
        is_fresh,
        is_noescape,
        is_owned,
        fn_params: Vec::new(),
        fn_param_effects: Vec::new(),
        fn_return: None,
        span: span.clone(),
    }
}

pub(in crate::rust_lower) fn simple_type_ref(name: &str, span: &Span) -> TypeRef {
    TypeRef {
        name: name.to_string(),
        args: Vec::new(),
        malformed_arg_spans: Vec::new(),
        is_fresh: false,
        is_noescape: false,
        is_owned: false,
        fn_params: Vec::new(),
        fn_param_effects: Vec::new(),
        fn_return: None,
        span: span.clone(),
    }
}

pub(in crate::rust_lower) fn substitute_type_ref(
    ty: &TypeRef,
    substitutions: &BTreeMap<String, TypeRef>,
) -> TypeRef {
    if ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && let Some(replacement) = substitutions.get(&ty.name)
    {
        let mut replacement = replacement.clone();
        replacement.is_fresh |= ty.is_fresh;
        replacement.is_noescape |= ty.is_noescape;
        replacement.is_owned |= ty.is_owned;
        return replacement;
    }

    let mut replaced = ty.clone();
    replaced.args = ty
        .args
        .iter()
        .map(|arg| substitute_type_ref(arg, substitutions))
        .collect();
    replaced.fn_params = ty
        .fn_params
        .iter()
        .map(|param| substitute_type_ref(param, substitutions))
        .collect();
    replaced.fn_return = ty
        .fn_return
        .as_ref()
        .map(|return_ty| Box::new(substitute_type_ref(return_ty, substitutions)));
    replaced
}

pub(in crate::rust_lower) fn collect_type_ref_substitutions(
    pattern: &TypeRef,
    actual: &TypeRef,
    type_params: &[String],
    substitutions: &mut BTreeMap<String, TypeRef>,
) {
    if pattern.args.is_empty()
        && pattern.fn_params.is_empty()
        && pattern.fn_return.is_none()
        && type_params.iter().any(|param| param == &pattern.name)
    {
        substitutions
            .entry(pattern.name.clone())
            .or_insert_with(|| actual.clone());
        return;
    }

    if pattern.name != actual.name || pattern.args.len() != actual.args.len() {
        return;
    }
    for (pattern_arg, actual_arg) in pattern.args.iter().zip(actual.args.iter()) {
        collect_type_ref_substitutions(pattern_arg, actual_arg, type_params, substitutions);
    }
    for (pattern_param, actual_param) in pattern.fn_params.iter().zip(actual.fn_params.iter()) {
        collect_type_ref_substitutions(pattern_param, actual_param, type_params, substitutions);
    }
    if let (Some(pattern_return), Some(actual_return)) =
        (pattern.fn_return.as_ref(), actual.fn_return.as_ref())
    {
        collect_type_ref_substitutions(pattern_return, actual_return, type_params, substitutions);
    }
}

/// Type names declared by the bundled stdlib (`builtin`) interfaces. These are
/// runtime-backed (lowered as `rsscript_runtime::X`), so they must be kept out of
/// the per-package `type_kinds` map even though they appear among the interface
/// programs — otherwise they would be reclassified as local user types. Parsed
/// once and cached (the builtin interface set is fixed at compile time).
pub(in crate::rust_lower) fn builtin_interface_type_names()
-> &'static std::collections::HashSet<String> {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<String>> =
        std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        crate::interfaces::builtin_interfaces()
            .flat_map(|(file, source)| crate::syntax::parse_source(file, source).items.into_iter())
            .filter_map(|item| match item {
                Item::Type(ty) => Some(ty.name),
                _ => None,
            })
            .collect()
    })
}

pub(in crate::rust_lower) fn fn_type_ref(
    params: Vec<TypeRef>,
    return_ty: Option<TypeRef>,
    span: &Span,
) -> TypeRef {
    TypeRef {
        name: "Fn".to_string(),
        args: Vec::new(),
        malformed_arg_spans: Vec::new(),
        is_fresh: false,
        is_noescape: true,
        is_owned: false,
        fn_param_effects: vec![None; params.len()],
        fn_params: params,
        fn_return: return_ty.map(Box::new),
        span: span.clone(),
    }
}

pub(in crate::rust_lower) fn is_json_encode_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Json" && type_root_name(name) == "encode"
    )
}

pub(in crate::rust_lower) fn is_json_decode_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Json"
                && matches!(type_root_name(name), "decode" | "decode_text")
    )
}

pub(in crate::rust_lower) fn is_json_decode_text_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Json" && type_root_name(name) == "decode_text"
    )
}

pub(in crate::rust_lower) fn json_decode_type_arg(callee: &Callee) -> Option<&str> {
    match callee {
        Callee::Qualified { name, .. } => {
            type_arg_names(name).and_then(|args| args.first().copied())
        }
        Callee::Name(name) => type_arg_names(name).and_then(|args| args.first().copied()),
        Callee::ReceiverCall { method, .. } => {
            type_arg_names(method).and_then(|args| args.first().copied())
        }
    }
}

pub(in crate::rust_lower) fn expr_is_json_literal(expr: &Expr) -> bool {
    match expr {
        Expr::ObjectLiteral { .. } | Expr::ArrayLiteral { .. } => true,
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => expr_is_json_literal(value),
        _ => false,
    }
}

pub(in crate::rust_lower) fn receiver_facade_namespace(
    receiver_root: &str,
    method: &str,
) -> Option<&'static str> {
    match receiver_root {
        "JsonValue" | "JsonLiteral" => Some("Json"),
        "String" if method.starts_with("json_") => Some("Json"),
        _ => None,
    }
}

pub(in crate::rust_lower) fn generated_line_count(generated: &str) -> usize {
    generated
        .chars()
        .filter(|character| *character == '\n')
        .count()
        + 1
}

pub(in crate::rust_lower) fn push_unique_derive(derives: &mut Vec<String>, derive: &str) {
    if !derives.iter().any(|existing| existing == derive) {
        derives.push(derive.to_string());
    }
}

/// Whether a type reference is (or directly is) a first-class closure value —
/// an `owned Fn(...)` — which lowers to `Rc<dyn Fn>`. Such a field makes the
/// containing type underivable for `Debug`/`Eq`/`Hash`/`Ord`.
pub(in crate::rust_lower) fn type_ref_holds_closure(ty: &TypeRef) -> bool {
    ty.is_owned && ty.name == "Fn"
}

/// A manual `Debug` impl for a struct that holds a non-`Debug` `owned Fn`
/// (`Rc<dyn Fn>`) field. Function-value fields print as `<fn>`; every other
/// field defers to its own `Debug`. Keeps the auto-`Debug` contract for
/// closure-holding structs (which cannot `#[derive(Debug)]`).
pub(in crate::rust_lower) fn manual_struct_debug_impl(
    name: &str,
    type_params: &[GenericParam],
    fields: &[FieldDecl],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "impl{} std::fmt::Debug for {}{} {{\n",
        lower_impl_generics(type_params),
        name,
        lower_generic_args(type_params)
    ));
    out.push_str("    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n");
    out.push_str(&format!("        f.debug_struct({name:?})\n"));
    for field in fields {
        let field_ident = rust_ident(&field.name);
        if type_ref_holds_closure(&field.ty) {
            out.push_str(&format!(
                "            .field({:?}, &\"<fn>\")\n",
                field.name
            ));
        } else {
            out.push_str(&format!(
                "            .field({:?}, &self.{field_ident})\n",
                field.name
            ));
        }
    }
    out.push_str("            .finish()\n");
    out.push_str("    }\n}\n");
    out
}

/// The Rust integer-literal suffix for an RSScript integer type name, e.g.
/// `Int -> "i64"`, `Int32 -> "i32"`. `None` for non-integer types. Used so an
/// integer literal in a typed context (sized ints, or the default `Int`) lowers
/// with the matching suffix instead of Rust's untyped-`i32` default.
pub(in crate::rust_lower) fn rust_int_literal_suffix(type_name: &str) -> Option<&'static str> {
    Some(match type_name {
        "Int" | "Int64" | "Fd" => "i64",
        "Int8" => "i8",
        "Int16" => "i16",
        "Int32" => "i32",
        "UInt" | "UInt64" => "u64",
        "UInt8" | "Byte" => "u8",
        "UInt16" => "u16",
        "UInt32" => "u32",
        _ => return None,
    })
}

pub(in crate::rust_lower) fn infer_const_type(expr: &Expr) -> String {
    match expr {
        Expr::Number(s, _) => {
            if s.contains('.') {
                "f64".to_string()
            } else {
                "i64".to_string()
            }
        }
        Expr::String(_, _) | Expr::MultilineString(_, _) => "&'static str".to_string(),
        Expr::CharLiteral(_, _) => "char".to_string(),
        Expr::Ident(name, _) if name == "true" || name == "false" => "bool".to_string(),
        // Non-literal const initializers are rejected by the frontend (RS0015); a
        // value reaching here means that check regressed — fail loudly.
        other => unreachable_lowering("const type", other.span()),
    }
}

pub(in crate::rust_lower) fn lower_const_value(expr: &Expr) -> String {
    match expr {
        Expr::Number(value, _) => value.clone(),
        Expr::String(value, _) => {
            format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
        }
        Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::CharLiteral(value, _) => format!("{:?}", decode_char_token(value)),
        Expr::Ident(name, _) if name == "true" || name == "false" => name.clone(),
        // The frontend rejects non-literal const initializers (RS0015), so anything
        // else here is a missing front-end check — fail loudly rather than emit a
        // `()` placeholder that leaks an unmappable backend type error.
        other => unreachable_lowering("const initializer", other.span()),
    }
}

pub(in crate::rust_lower) fn is_try_wrapped(expr: &Expr) -> bool {
    matches!(expr, Expr::Try { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_util::builtin_generic_type_params;

    #[test]
    pub(in crate::rust_lower) fn generic_substitution_uses_declared_parameter_names() {
        let span = Span {
            file: "generic-substitution.rss".to_string(),
            line: 1,
            column: 1,
            length: 1,
        };
        let pattern = type_ref_from_display("Result<U, List<W>>", &span);
        let actual = type_ref_from_display("Result<Int, List<String>>", &span);
        let mut substitutions = BTreeMap::new();

        collect_type_ref_substitutions(
            &pattern,
            &actual,
            &["U".to_string(), "W".to_string()],
            &mut substitutions,
        );

        assert_eq!(
            substitutions.get("U").map(|ty| ty.name.as_str()),
            Some("Int")
        );
        assert_eq!(
            substitutions.get("W").map(|ty| ty.name.as_str()),
            Some("String")
        );
        assert!(!substitutions.contains_key("T"));
    }

    #[test]
    pub(in crate::rust_lower) fn builtin_generic_type_params_use_each_type_s_own_param_names() {
        // Regression: `Result` was mapped to `["K", "V"]` (Map's params), so the
        // namespace/type-argument substitution path never bound `Result`'s real
        // `T`/`E` params — weakening generic substitution for `Result.map` /
        // `map_error` / `and_then`. Each type must use its own declared param names.
        assert_eq!(builtin_generic_type_params("Map"), Some(vec!["K", "V"]));
        assert_eq!(builtin_generic_type_params("Result"), Some(vec!["T", "E"]));
        assert_eq!(builtin_generic_type_params("List"), Some(vec!["T"]));
        assert_eq!(builtin_generic_type_params("Option"), Some(vec!["T"]));
        assert_eq!(builtin_generic_type_params("NotAGeneric"), None);
    }

    #[test]
    pub(in crate::rust_lower) fn float_arithmetic_is_not_reclassified_as_int_during_lowering() {
        let source = r#"
protocol Numeric {
    pub(in crate::rust_lower) fn value(self: read Self) -> Float
}

fn Float.value(self: read Float) -> Float {
    return self
}

impl Numeric for Float {
    value = Float.value
}

fn make() -> Capability<Numeric> {
    let number = 1.0 + 2.0
    return Capability<Numeric>.from(value: take number)
}
"#;
        let program = crate::syntax::parse_source("float-arithmetic-type.rss", source);
        let rust = crate::rust_lower::lower_program_to_rust(&program);

        assert!(rust.contains("CapabilityNumeric::Float(number)"), "{rust}");
        assert!(!rust.contains("CapabilityNumeric::Int("), "{rust}");
    }
}
