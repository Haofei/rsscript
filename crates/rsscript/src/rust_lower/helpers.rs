mod executable_declarations;
mod semantic_projection;

pub(super) use crate::text_util::{
    decode_char_token, decode_string_token, type_arg_names, type_root_name,
};
pub(super) use executable_declarations::*;
pub(super) use semantic_projection::*;
use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Span;
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FileFeature, FunctionDecl, GenericBound,
    GenericParam, Item, MatchLiteral, MatchPattern, Param, Program, Stmt, TypeRef,
};

use super::intrinsics::*;
use super::lowerer::RustLowerer;

pub(super) fn explicit_weak_handle_source(expr: &Expr) -> Option<&Expr> {
    let Expr::Call { callee, args, .. } = expr else {
        return None;
    };
    if !matches!(
        callee,
        Callee::Qualified { namespace, name }
            if namespace == "Weak" && matches!(name.as_str(), "from" | "downgrade")
    ) {
        return None;
    }
    match args.as_slice() {
        [arg] if arg.name.as_deref() == Some("value") => Some(&arg.value),
        _ => None,
    }
}

pub(super) fn is_weak_upgrade_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "Weak" && type_root_name(name) == "upgrade"
    )
}

pub(super) fn is_weak_from_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if namespace == "Weak" && matches!(type_root_name(name), "from" | "downgrade")
    )
}

pub(super) fn lower_weak_from_call(lowerer: &mut RustLowerer<'_>, args: &[CallArg]) -> String {
    let Some(arg) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("value"))
        .or_else(|| args.first())
    else {
        return "rsscript_runtime::weak(&/* missing value */)".to_string();
    };
    lowerer.lower_runtime_weak_from_managed(&arg.value)
}

pub(super) fn lower_weak_upgrade_call(lowerer: &mut RustLowerer<'_>, args: &[CallArg]) -> String {
    let Some(arg) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("value"))
        .or_else(|| args.first())
    else {
        return "None".to_string();
    };
    let target = match &arg.value {
        Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } => lowerer.lower_expr(value),
        _ => lowerer.lower_expr(&arg.value),
    };
    format!("{target}.upgrade()")
}

pub(super) fn is_result_type(ty: &TypeRef) -> bool {
    ty.name == "Result" && ty.args.len() == 2
}

pub(super) fn fn_type_return(ty: &TypeRef) -> Option<&TypeRef> {
    if ty.name == "Fn" {
        ty.fn_return.as_deref()
    } else {
        None
    }
}

pub(super) fn list_element_type_ref(ty: &TypeRef) -> Option<TypeRef> {
    if ty.name == "List" && ty.args.len() == 1 {
        ty.args.first().cloned()
    } else {
        None
    }
}

pub(super) fn stream_item_type_ref(ty: &TypeRef) -> Option<TypeRef> {
    if ty.name == "Stream" && ty.args.len() == 1 {
        ty.args.first().cloned()
    } else {
        None
    }
}

pub(super) fn is_copy_type_ref(ty: &TypeRef) -> bool {
    ty.args.is_empty()
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

pub(super) fn is_result_constructor_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Call {
            callee: Callee::Name(name),
            ..
        } => matches!(name.as_str(), "Ok" | "Err"),
        _ => false,
    }
}

pub(super) fn collect_function_return_types(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeMap<String, TypeRef> {
    let mut return_types = BTreeMap::new();
    collect_program_function_return_types(program, &mut return_types);
    for interface_program in interface_programs {
        collect_program_function_return_types(interface_program, &mut return_types);
    }
    return_types
}

pub(super) fn collect_function_retained_params(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut retained_params = BTreeMap::new();
    for interface_program in interface_programs {
        collect_program_function_retained_params(interface_program, &mut retained_params);
    }
    collect_program_function_retained_params(program, &mut retained_params);
    retained_params
}

pub(super) fn collect_program_function_retained_params(
    program: &Program,
    retained_params: &mut BTreeMap<String, BTreeSet<String>>,
) {
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let retained = function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Retains(param) => Some(param.clone()),
                EffectDecl::Name(_) => None,
            })
            .collect::<BTreeSet<_>>();
        if !retained.is_empty() {
            retained_params.insert(native_boundary_function_key(&function.name), retained);
        }
    }
}

pub(super) fn collect_program_function_return_types(
    program: &Program,
    return_types: &mut BTreeMap<String, TypeRef>,
) {
    for item in &program.items {
        if let Item::Function(function) = item
            && let Some(return_ty) = &function.return_ty
        {
            return_types.insert(function.name.clone(), return_ty.clone());
        }
    }
}

pub(super) fn collect_native_boundary_callees(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeSet<String> {
    let mut callees = BTreeSet::new();
    for interface in interface_programs {
        collect_native_boundary_callees_from_program(interface, &mut callees);
    }
    collect_native_boundary_callees_from_program(program, &mut callees);
    callees
}

pub(super) fn collect_async_native_boundary_callees(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeSet<String> {
    let mut callees = BTreeSet::new();
    for interface in interface_programs {
        collect_async_native_boundary_callees_from_program(interface, &mut callees);
    }
    collect_async_native_boundary_callees_from_program(program, &mut callees);
    callees
}

pub(super) fn collect_native_boundary_callees_from_program(
    program: &Program,
    callees: &mut BTreeSet<String>,
) {
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if function.effects.iter().any(is_native_boundary) {
            callees.insert(native_boundary_function_key(&function.name));
        }
    }
}

pub(super) fn collect_async_native_boundary_callees_from_program(
    program: &Program,
    callees: &mut BTreeSet<String>,
) {
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if function.is_async && function.effects.iter().any(is_native_boundary) {
            callees.insert(native_boundary_function_key(&function.name));
        }
    }
}

pub(super) fn native_boundary_function_key(name: &str) -> String {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        format!("{}.{}", type_root_name(namespace), name)
    } else {
        name.to_string()
    }
}

pub(super) fn native_boundary_callee_key(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => type_root_name(name).to_string(),
        Callee::Qualified { namespace, name } => {
            format!("{}.{}", type_root_name(namespace), type_root_name(name))
        }
        Callee::ReceiverCall { method, .. } => type_root_name(method).to_string(),
    }
}

pub(super) fn collect_mutated_bindings(block: &Block) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect_mutated_bindings_from_block(block, &mut names);
    names
}

/// The awaited expression of `await x` or `await x?`, else `None`.
pub(super) fn async_await_inner(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Try { value, .. } => match value.as_ref() {
            Expr::Await { value, .. } => Some(value),
            _ => None,
        },
        Expr::Await { value, .. } => Some(value),
        _ => None,
    }
}

/// Whether the await form carries a `?` (`await x?`), so the continuation should
/// short-circuit on `Err`.
pub(super) fn async_await_is_try(expr: &Expr) -> bool {
    matches!(expr, Expr::Try { value, .. } if matches!(value.as_ref(), Expr::Await { .. }))
}

pub(super) fn block_needs_async_executor(block: &Block) -> bool {
    block.statements.iter().any(stmt_needs_async_executor)
}

fn stmt_needs_async_executor(statement: &Stmt) -> bool {
    match statement {
        Stmt::TaskGroup(_) | Stmt::Select(_) => true,
        Stmt::With(stmt) => block_needs_async_executor(&stmt.body),
        Stmt::If(stmt) => {
            block_needs_async_executor(&stmt.then_body)
                || stmt
                    .else_body
                    .as_ref()
                    .is_some_and(block_needs_async_executor)
        }
        Stmt::Loop(stmt) => block_needs_async_executor(&stmt.body),
        Stmt::For(stmt) => stmt.is_async || block_needs_async_executor(&stmt.body),
        Stmt::Match(stmt) => stmt
            .arms
            .iter()
            .any(|arm| block_needs_async_executor(&arm.body)),
        Stmt::LetElse(stmt) => block_needs_async_executor(&stmt.else_body),
        Stmt::Let(_)
        | Stmt::Return(_)
        | Stmt::Assign(_)
        | Stmt::Expr(_)
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

pub(super) fn collect_mutated_bindings_from_block(block: &Block, names: &mut BTreeSet<String>) {
    for statement in &block.statements {
        collect_mutated_bindings_from_stmt(statement, names);
    }
}

pub(super) fn collect_mutated_bindings_from_stmt(statement: &Stmt, names: &mut BTreeSet<String>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_mutated_bindings_from_expr(value, names);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_mutated_bindings_from_expr(value, names);
            }
        }
        Stmt::With(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.resource, names);
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::If(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.condition, names);
            collect_mutated_bindings_from_block(&stmt.then_body, names);
            if let Some(else_body) = &stmt.else_body {
                collect_mutated_bindings_from_block(else_body, names);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_mutated_bindings_from_expr(condition, names);
            }
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::For(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.iterable, names);
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::TaskGroup(stmt) => {
            collect_mutated_bindings_from_block(&stmt.body, names);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_mutated_bindings_from_expr(&arm.operation, names);
                collect_mutated_bindings_from_block(&arm.body, names);
            }
        }
        Stmt::Match(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.value, names);
            for arm in &stmt.arms {
                collect_mutated_bindings_from_block(&arm.body, names);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_mutated_bindings_from_expr(&stmt.value, names);
            collect_mutated_bindings_from_block(&stmt.else_body, names);
        }
        Stmt::Assign(stmt) => {
            // The assigned place's root local must be `let mut` in generated Rust.
            if let Some(name) = mutable_root_ident(&stmt.target) {
                names.insert(name.to_string());
            }
            collect_mutated_bindings_from_expr(&stmt.target, names);
            collect_mutated_bindings_from_expr(&stmt.value, names);
        }
        Stmt::Expr(expr) => collect_mutated_bindings_from_expr(expr, names),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

pub(super) fn collect_mutated_bindings_from_expr(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_mutated_bindings_from_expr(left, names);
            collect_mutated_bindings_from_expr(right, names);
        }
        Expr::Field { base, .. } => collect_mutated_bindings_from_expr(base, names),
        Expr::Index { base, index, .. } => {
            collect_mutated_bindings_from_expr(base, names);
            collect_mutated_bindings_from_expr(index, names);
        }
        Expr::Call { callee, args, .. } => {
            if let Callee::ReceiverCall {
                receiver, effect, ..
            } = callee
                && (*effect) == Some(DataEffect::Mut)
                && let Some(root) = expr_root_name(receiver)
            {
                names.insert(root.to_string());
            }
            for arg in args {
                collect_mutated_bindings_from_expr(&arg.value, names);
            }
        }
        Expr::Effect { effect, value, .. } => {
            if *effect == DataEffect::Mut
                && let Some(name) = mutable_root_ident(value)
            {
                names.insert(name.to_string());
            }
            collect_mutated_bindings_from_expr(value, names);
        }
        Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            collect_mutated_bindings_from_expr(value, names);
        }
        Expr::Closure { body, .. } => collect_mutated_bindings_from_block(body, names),
        Expr::Match { value, arms, .. } => {
            collect_mutated_bindings_from_expr(value, names);
            for arm in arms {
                collect_mutated_bindings_from_block(&arm.body, names);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::MapLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn expr_root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Field { base, .. } | Expr::Index { base, .. } => expr_root_name(base),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            expr_root_name(value)
        }
        _ => None,
    }
}

pub(super) fn closure_value_mutates_capture(expr: &Expr) -> bool {
    let Expr::Closure { body, .. } = expr else {
        return false;
    };
    let mut bound = BTreeSet::new();
    collect_closure_bound_names_from_block(body, &mut bound);
    closure_block_mutates_unbound_name(body, &bound)
}

pub(super) fn collect_closure_bound_names_from_block(block: &Block, names: &mut BTreeSet<String>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                names.insert(stmt.name.clone());
            }
            Stmt::With(stmt) => {
                names.insert(stmt.binding.clone());
                collect_closure_bound_names_from_block(&stmt.body, names);
            }
            Stmt::If(stmt) => {
                collect_closure_bound_names_from_block(&stmt.then_body, names);
                if let Some(else_body) = &stmt.else_body {
                    collect_closure_bound_names_from_block(else_body, names);
                }
            }
            Stmt::Loop(stmt) => collect_closure_bound_names_from_block(&stmt.body, names),
            Stmt::For(stmt) => {
                names.insert(stmt.binding.clone());
                collect_closure_bound_names_from_block(&stmt.body, names);
            }
            Stmt::TaskGroup(stmt) => {
                collect_closure_bound_names_from_block(&stmt.body, names);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    collect_closure_bound_names_from_block(&arm.body, names);
                }
            }
            Stmt::Match(stmt) => {
                for arm in &stmt.arms {
                    collect_closure_bound_names_from_block(&arm.body, names);
                }
            }
            Stmt::LetElse(stmt) => {
                if !stmt.binding_name.is_empty() {
                    names.insert(stmt.binding_name.clone());
                }
                collect_closure_bound_names_from_block(&stmt.else_body, names);
            }
            Stmt::Return(_)
            | Stmt::Assign(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Expr(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

pub(super) fn closure_block_mutates_unbound_name(block: &Block, bound: &BTreeSet<String>) -> bool {
    block
        .statements
        .iter()
        .any(|statement| closure_stmt_mutates_unbound_name(statement, bound))
}

pub(super) fn closure_stmt_mutates_unbound_name(
    statement: &Stmt,
    bound: &BTreeSet<String>,
) -> bool {
    match statement {
        Stmt::Let(stmt) => stmt
            .value
            .as_ref()
            .is_some_and(|value| closure_expr_mutates_unbound_name(value, bound)),
        Stmt::Return(stmt) => stmt
            .value
            .as_ref()
            .is_some_and(|value| closure_expr_mutates_unbound_name(value, bound)),
        Stmt::With(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.resource, bound)
                || closure_block_mutates_unbound_name(&stmt.body, bound)
        }
        Stmt::If(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.condition, bound)
                || closure_block_mutates_unbound_name(&stmt.then_body, bound)
                || stmt
                    .else_body
                    .as_ref()
                    .is_some_and(|body| closure_block_mutates_unbound_name(body, bound))
        }
        Stmt::Loop(stmt) => {
            stmt.condition
                .as_ref()
                .is_some_and(|condition| closure_expr_mutates_unbound_name(condition, bound))
                || closure_block_mutates_unbound_name(&stmt.body, bound)
        }
        Stmt::For(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.iterable, bound)
                || closure_block_mutates_unbound_name(&stmt.body, bound)
        }
        Stmt::TaskGroup(stmt) => closure_block_mutates_unbound_name(&stmt.body, bound),
        Stmt::Select(stmt) => stmt.arms.iter().any(|arm| {
            closure_expr_mutates_unbound_name(&arm.operation, bound)
                || closure_block_mutates_unbound_name(&arm.body, bound)
        }),
        Stmt::Match(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.value, bound)
                || stmt
                    .arms
                    .iter()
                    .any(|arm| closure_block_mutates_unbound_name(&arm.body, bound))
        }
        Stmt::LetElse(stmt) => {
            closure_expr_mutates_unbound_name(&stmt.value, bound)
                || closure_block_mutates_unbound_name(&stmt.else_body, bound)
        }
        Stmt::Assign(stmt) => {
            mutable_root_ident(&stmt.target).is_some_and(|name| !bound.contains(name))
                || closure_expr_mutates_unbound_name(&stmt.value, bound)
        }
        Stmt::Expr(expr) => closure_expr_mutates_unbound_name(expr, bound),
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

pub(super) fn closure_expr_mutates_unbound_name(expr: &Expr, bound: &BTreeSet<String>) -> bool {
    match expr {
        Expr::Effect {
            effect: DataEffect::Mut,
            value,
            ..
        } => {
            mutable_root_ident(value).is_some_and(|name| !bound.contains(name))
                || closure_expr_mutates_unbound_name(value, bound)
        }
        Expr::Binary { left, right, .. } => {
            closure_expr_mutates_unbound_name(left, bound)
                || closure_expr_mutates_unbound_name(right, bound)
        }
        Expr::Field { base, .. } => closure_expr_mutates_unbound_name(base, bound),
        Expr::Index { base, index, .. } => {
            closure_expr_mutates_unbound_name(base, bound)
                || closure_expr_mutates_unbound_name(index, bound)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| closure_expr_mutates_unbound_name(&arg.value, bound)),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => closure_expr_mutates_unbound_name(value, bound),
        Expr::Closure { body, .. } => closure_block_mutates_unbound_name(body, bound),
        Expr::Match { value, arms, .. } => {
            closure_expr_mutates_unbound_name(value, bound)
                || arms
                    .iter()
                    .any(|arm| closure_block_mutates_unbound_name(&arm.body, bound))
        }
        Expr::ObjectLiteral { fields, .. } => fields
            .iter()
            .any(|field| closure_expr_mutates_unbound_name(&field.value, bound)),
        Expr::MapLiteral { entries, .. } => entries.iter().any(|entry| {
            closure_expr_mutates_unbound_name(&entry.key, bound)
                || closure_expr_mutates_unbound_name(&entry.value, bound)
        }),
        Expr::ArrayLiteral { items, .. } => items
            .iter()
            .any(|item| closure_expr_mutates_unbound_name(item, bound)),
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => false,
    }
}

pub(super) fn mutable_root_ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => mutable_root_ident(base),
        _ => None,
    }
}

pub(super) fn stmt_span(statement: &Stmt) -> &Span {
    match statement {
        Stmt::Let(stmt) => &stmt.span,
        Stmt::Return(stmt) => &stmt.span,
        Stmt::With(stmt) => &stmt.span,
        Stmt::If(stmt) => &stmt.span,
        Stmt::Loop(stmt) => &stmt.span,
        Stmt::For(stmt) => &stmt.span,
        Stmt::TaskGroup(stmt) => &stmt.span,
        Stmt::Select(stmt) => &stmt.span,
        Stmt::Match(stmt) => &stmt.span,
        Stmt::LetElse(stmt) => &stmt.span,
        Stmt::Break(span)
        | Stmt::Continue(span)
        | Stmt::MalformedWith(span)
        | Stmt::MalformedIf(span)
        | Stmt::MalformedLoop(span)
        | Stmt::MalformedFor(span)
        | Stmt::MalformedMatch(span)
        | Stmt::Unknown(span) => span,
        Stmt::Assign(stmt) => &stmt.span,
        Stmt::Expr(expr) => expr.span(),
    }
}

pub(super) fn lower_match_pattern(pattern: &MatchPattern) -> String {
    match pattern {
        MatchPattern::Binding { name, .. } => rust_ident(name),
        MatchPattern::Wildcard(_) => "_".to_string(),
        MatchPattern::Literal { value, .. } => lower_match_literal(value),
        MatchPattern::Variant { name, bindings, .. } if !bindings.is_empty() => {
            let parts = bindings
                .iter()
                .map(lower_match_pattern)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({parts})", rust_ident(name))
        }
        MatchPattern::Variant { name, .. } if matches!(name.as_str(), "Some" | "Ok" | "Err") => {
            format!("{}(_)", rust_ident(name))
        }
        MatchPattern::Variant { name, .. } => rust_ident(name),
        MatchPattern::Struct {
            name,
            fields,
            has_rest,
            ..
        } => lower_struct_match_pattern(None, name, fields, *has_rest),
        MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } => lower_list_match_pattern(prefix, rest.as_ref(), suffix),
    }
}

/// Render a list slice pattern as a native Rust slice pattern: `[]`, `[a, b]`,
/// `[first, rest @ ..]`, `[init @ .., last]`. Element/rest bindings come out as
/// references (`&T` / `&[T]`); callers that need owned values add clone
/// rebindings.
pub(super) fn lower_list_match_pattern(
    prefix: &[MatchPattern],
    rest: Option<&Option<String>>,
    suffix: &[MatchPattern],
) -> String {
    let mut parts: Vec<String> = prefix.iter().map(lower_match_pattern).collect();
    if let Some(rest_binding) = rest {
        match rest_binding {
            Some(name) => parts.push(format!("{} @ ..", rust_ident(name))),
            None => parts.push("..".to_string()),
        }
    }
    parts.extend(suffix.iter().map(lower_match_pattern));
    format!("[{}]", parts.join(", "))
}

fn lower_struct_match_pattern(
    namespace: Option<&str>,
    name: &str,
    fields: &[crate::syntax::ast::MatchFieldPattern],
    has_rest: bool,
) -> String {
    let path = if let Some(namespace) = namespace {
        format!("{}::{}", rust_ident(namespace), rust_ident(name))
    } else {
        rust_ident(name)
    };
    let mut parts = Vec::new();
    for field in fields {
        if field.ignored {
            parts.push(format!("{}: _", rust_ident(&field.name)));
        } else if let Some(pattern) = &field.pattern {
            parts.push(format!(
                "{}: {}",
                rust_ident(&field.name),
                lower_match_pattern(pattern)
            ));
        } else if let Some(binding) = &field.binding {
            let binding_text = if field.effect == Some(crate::syntax::ast::DataEffect::Mut) {
                format!("mut {}", rust_ident(binding))
            } else {
                rust_ident(binding)
            };
            if binding == &field.name {
                if field.effect == Some(crate::syntax::ast::DataEffect::Mut) {
                    parts.push(format!("{}: {binding_text}", rust_ident(&field.name)));
                } else {
                    parts.push(rust_ident(&field.name));
                }
            } else {
                parts.push(format!("{}: {}", rust_ident(&field.name), binding_text));
            }
        }
    }
    if has_rest {
        parts.push("..".to_string());
    }
    format!("{path} {{ {} }}", parts.join(", "))
}

pub(super) fn lower_match_literal(value: &MatchLiteral) -> String {
    match value {
        MatchLiteral::Int(value) => value.clone(),
        MatchLiteral::String(value) => format!("{:?}", decode_string_token(value)),
        MatchLiteral::Char(value) => format!("{:?}", decode_char_token(value)),
        MatchLiteral::Bool(value) => value.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ManagedPosition {
    Bare,
    Param,
    Return,
    FreshReturn,
    Nested,
}

pub(super) fn visibility(is_public: bool) -> &'static str {
    if is_public { "pub " } else { "" }
}

pub(super) fn lower_generic_params(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }

    let params = params
        .iter()
        .map(|param| {
            let name = rust_ident(&param.name);
            // Plain value-type generics are cloned by RSScript's value semantics
            // (e.g. `List.get<T>`), so they need `Clone` in generated Rust. Bounded
            // generics keep their declared bound: protocol/managed/resource
            // implementors aren't necessarily `Clone`, and over-constraining them
            // (e.g. a `Writer`) would reject legitimate callers.
            match &param.bound {
                Some(GenericBound::Managed) => format!("{name}: rsscript_runtime::ManagedValue"),
                Some(GenericBound::Struct) => name,
                Some(GenericBound::Resource) => format!("{name}: rsscript_runtime::Resource"),
                Some(GenericBound::Protocol(protocol)) if protocol == "Ord" => {
                    format!("{name}: std::cmp::Ord")
                }
                Some(GenericBound::Protocol(protocol)) => {
                    format!("{name}: {}", rust_ident(protocol))
                }
                None => format!("{name}: Clone"),
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{params}>")
}

pub(super) fn lower_impl_generics(params: &[GenericParam]) -> String {
    lower_generic_params(params)
}

pub(super) fn lower_generic_args(params: &[GenericParam]) -> String {
    if params.is_empty() {
        return String::new();
    }

    let args = params
        .iter()
        .map(|param| rust_value_ident(&param.name))
        .collect::<Vec<_>>()
        .join(", ");
    format!("<{args}>")
}

pub(super) fn is_native_boundary(effect: &EffectDecl) -> bool {
    matches!(effect, EffectDecl::Name(name) if matches!(name.as_str(), "native" | "unsafe"))
}

pub(super) fn lower_callee(callee: &Callee) -> String {
    if let Some(target) = runtime_intrinsic_target(callee) {
        return target.to_string();
    }

    match callee {
        Callee::Name(name) => {
            let canonical = type_root_name(name);
            lower_name_override(canonical).unwrap_or_else(|| rust_ident(canonical))
        }
        Callee::Qualified { namespace, name } => rust_qualified_function_ident(namespace, name),
        Callee::ReceiverCall { method, .. } => rust_ident(type_root_name(method)),
    }
}

pub(super) fn lower_protocol_callee(callee: &Callee) -> String {
    match callee {
        Callee::Qualified { namespace, name } => {
            format!("{}::{}", rust_ident(namespace), rust_ident(name))
        }
        Callee::Name(name) => rust_ident(name),
        Callee::ReceiverCall { method, .. } => rust_ident(method),
    }
}

pub(super) fn protocol_impl_forward_arg(param: &Param) -> String {
    if param.name == "self" {
        return match param.effective_effect() {
            Some(DataEffect::Read) => "self".to_string(),
            Some(DataEffect::Mut) => "self".to_string(),
            Some(DataEffect::Take) | None => "self".to_string(),
        };
    }
    rust_value_ident(&param.name)
}

pub(super) fn protocol_method_keys(program: &Program) -> BTreeSet<String> {
    program
        .protocols
        .iter()
        .flat_map(|protocol| {
            protocol_methods(program, &protocol.name)
                .into_iter()
                .map(|method| executable_declaration_function_key(&method.name))
        })
        .collect()
}

pub(super) fn protocol_methods<'a>(program: &'a Program, protocol: &str) -> Vec<&'a FunctionDecl> {
    program
        .items
        .iter()
        .filter_map(|item| {
            let Item::Function(function) = item else {
                return None;
            };
            is_protocol_method(function, protocol).then_some(function)
        })
        .collect()
}

pub(super) fn protocol_method<'a>(
    program: &'a Program,
    protocol: &str,
    method_name: &str,
) -> Option<&'a FunctionDecl> {
    protocol_methods(program, protocol)
        .into_iter()
        .find(|method| protocol_method_name(&method.name) == method_name)
}

pub(super) fn is_protocol_method(function: &FunctionDecl, protocol: &str) -> bool {
    function
        .name
        .rsplit_once('.')
        .is_some_and(|(namespace, _)| namespace == protocol)
}

pub(super) fn protocol_method_name(name: &str) -> &str {
    name.rsplit_once('.')
        .map(|(_, method)| method)
        .unwrap_or(name)
}

pub(super) fn is_file_open_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if is_file_open_callee(callee) || is_file_open_read_callee(callee) || is_file_open_write_callee(callee))
}

pub(super) fn lower_string_concat_call(lowerer: &mut RustLowerer<'_>, args: &[CallArg]) -> String {
    let left = lower_call_arg(lowerer, args, "left", 0, "\"\".to_string()");
    let right = lower_call_arg(lowerer, args, "right", 1, "\"\".to_string()");
    format!("format!(\"{{}}{{}}\", {left}, {right})")
}

pub(super) fn lower_call_arg(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    name: &str,
    index: usize,
    default: &str,
) -> String {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .or_else(|| args.get(index))
        .map(|arg| lowerer.lower_expr(&arg.value))
        .unwrap_or_else(|| default.to_string())
}

pub(super) fn lower_builtin_value_ident(name: &str) -> Option<&'static str> {
    match name {
        "Unit" => Some("()"),
        "true" => Some("true"),
        "false" => Some("false"),
        "None" => Some("None"),
        _ => None,
    }
}

// Re-exported from the shared text utilities (single source of truth); kept at
// `super` visibility so the rest of `rust_lower` reaches it via `helpers::*`.

pub(super) fn is_rust_enum_constructor(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some")
}

pub(super) fn runtime_struct_constructor(
    name: &str,
) -> Option<(&'static str, &'static [&'static str])> {
    match name {
        "ProcessEnv" => Some(("rsscript_runtime::ProcessEnv", &["name", "value"])),
        "ProcessEvent" => Some((
            "rsscript_runtime::ProcessEvent",
            &["kind", "data", "status"],
        )),
        "ProcessRequest" => Some((
            "rsscript_runtime::ProcessRequest",
            &[
                "command",
                "args",
                "cwd",
                "stdin",
                "env",
                "timeout_ms",
                "merge_stderr",
                "output_cap_bytes",
            ],
        )),
        _ => None,
    }
}

pub(super) fn lower_source_span(span: &Span) -> String {
    format!(
        "rsscript_runtime::SourceSpan::new({:?}, {}, {}, {})",
        span.file, span.line, span.column, span.length
    )
}

thread_local! {
    /// Per-lowering-run map from a function's source qualified name (e.g.
    /// `helpers.count`) to its pinned backend name from `#lower_name("...")`.
    /// Populated before a lowering run (and before building the symbol inventory)
    /// and consulted by the canonical name-lowering helpers so the emitted Rust
    /// symbol and the reported `lowered_name` always agree.
    static LOWER_NAME_OVERRIDES: std::cell::RefCell<std::collections::HashMap<String, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Install the pinned-name overrides for the current thread; pass an empty map to
/// disable. Returns the previous map so callers can restore it.
pub(crate) fn set_lower_name_overrides(
    overrides: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    LOWER_NAME_OVERRIDES.with(|cell| cell.replace(overrides))
}

fn lower_name_override(source_name: &str) -> Option<String> {
    LOWER_NAME_OVERRIDES.with(|cell| cell.borrow().get(source_name).cloned())
}

pub(super) fn rust_function_ident(name: &str) -> String {
    if let Some(pinned) = lower_name_override(name) {
        return pinned;
    }
    let joined = name.split('.').collect::<Vec<_>>().join("_");
    rust_ident(&joined)
}

pub(super) fn rust_qualified_function_ident(namespace: &str, name: &str) -> String {
    let source_name = format!("{namespace}.{}", type_root_name(name));
    if let Some(pinned) = lower_name_override(&source_name) {
        return pinned;
    }
    namespace
        .split('.')
        .chain(std::iter::once(type_root_name(name)))
        .map(rust_path_segment)
        .collect::<Vec<_>>()
        .join("_")
}

pub(super) fn rust_path_segment(segment: &str) -> String {
    if let Some((head, tail)) = segment.split_once("::<") {
        format!("{}::<{}", rust_ident(head), tail)
    } else {
        rust_ident(segment)
    }
}

/// Whether `name` is a Rust keyword (strict or reserved) that cannot be used as a
/// bare identifier.
pub(crate) fn is_rust_keyword(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
        "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
        "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
        "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "try",
        "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
    ];
    KEYWORDS.contains(&name)
}

pub(super) fn rust_ident(name: &str) -> String {
    if is_rust_keyword(name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

pub(super) fn rust_value_ident(name: &str) -> String {
    if name == "self" {
        "rss_self".to_string()
    } else {
        rust_ident(name)
    }
}

pub(super) fn cargo_package_name(name: &str) -> String {
    let mut out = String::new();
    let mut previous_was_dash = false;
    let mut previous_was_lower_or_digit = false;

    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase()
                && previous_was_lower_or_digit
                && !previous_was_dash
                && !out.is_empty()
            {
                out.push('-');
            }
            out.push(character.to_ascii_lowercase());
            previous_was_dash = false;
            previous_was_lower_or_digit =
                character.is_ascii_lowercase() || character.is_ascii_digit();
        } else if (character.is_ascii_whitespace() || matches!(character, '-' | '_' | '.'))
            && !out.is_empty()
            && !previous_was_dash
        {
            out.push('-');
            previous_was_dash = true;
            previous_was_lower_or_digit = false;
        } else {
            previous_was_lower_or_digit = false;
        }
    }

    while out.ends_with('-') {
        out.pop();
    }

    if out.is_empty() {
        "rsscript-generated".to_string()
    } else if is_rust_keyword(&out) {
        // A package named after a Rust keyword (e.g. `async.rss`) would derive a
        // keyword crate/lib name, which both Cargo and rustc reject (and the
        // generated `async::main()` harness is a parse error). Prefix it so the
        // package name, lib name, and `<crate>::main()` reference are all valid.
        format!("rss-{out}")
    } else {
        out
    }
}

pub(super) fn rust_package_main(program: &Program, package_name: &str) -> Option<String> {
    let main = program.items.iter().find_map(|item| match item {
        Item::Function(function) => runnable_main_kind(function).map(|kind| (function, kind)),
        Item::Type(_)
        | Item::Module(_)
        | Item::Use(_)
        | Item::SumType(_)
        | Item::TypeAlias(_)
        | Item::Const(_) => None,
    })?;
    let (main, kind) = main;
    let crate_name = cargo_crate_name(package_name);
    let call = match kind {
        RunnableMainKind::Unit => format!("{}::{}();", crate_name, rust_ident(&main.name)),
        // A `main` returning `Err` is a failed run on every backend (ledger
        // SH-005): report it to stderr and exit non-zero, rather than panicking.
        RunnableMainKind::ResultUnit => format!(
            "if let Err(error) = {}::{}() {{ \
             eprintln!(\"RSScript main returned an error: {{error:?}}\"); \
             std::process::exit(1); }}",
            crate_name,
            rust_ident(&main.name)
        ),
    };
    Some(format!(
        concat!(
            "// Generated by RSScript. Edit the .rss source instead.\n",
            "// Runnable harness for RSScript `{}`.\n\n",
            "fn main() {{\n",
            "    rsscript_runtime::install_runtime_diagnostic_panic_hook();\n",
            "    {}\n",
            "}}\n"
        ),
        main.name, call
    ))
}

pub(super) fn lowered_feature_names(features: &[FileFeature]) -> Vec<&'static str> {
    let mut names = features
        .iter()
        .map(|feature| match feature {
            FileFeature::Local => "local",
            FileFeature::Native => "native",
            FileFeature::Unsafe => "unsafe",
            FileFeature::Async => "async",
            FileFeature::Device => "device",
            FileFeature::Ffi => "ffi",
            FileFeature::Reflection => "reflection",
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
}

pub(super) fn is_runnable_main(function: &FunctionDecl) -> bool {
    runnable_main_kind(function).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunnableMainKind {
    Unit,
    ResultUnit,
}

pub(super) fn runnable_main_kind(function: &FunctionDecl) -> Option<RunnableMainKind> {
    if function.name != "main" || !function.params.is_empty() {
        return None;
    }
    let Some(return_ty) = function.return_ty.as_ref() else {
        return Some(RunnableMainKind::Unit);
    };
    if return_ty.name == "Unit" && return_ty.args.is_empty() {
        return Some(RunnableMainKind::Unit);
    }
    if return_ty.name == "Result"
        && return_ty.args.len() == 2
        && return_ty.args[0].name == "Unit"
        && return_ty.args[0].args.is_empty()
    {
        return Some(RunnableMainKind::ResultUnit);
    }
    None
}

pub(super) fn unreachable_lowering(kind: &str, span: &Span) -> ! {
    panic!(
        "internal RSScript lowering error: unsupported {kind} reached Rust lowering at {}:{}:{}",
        span.file, span.line, span.column
    )
}

pub(super) fn result_ok_type_ref(ty: &TypeRef) -> Option<TypeRef> {
    if ty.name != "Result" {
        return None;
    }
    let mut ok_ty = ty.args.first()?.clone();
    ok_ty.is_fresh = false;
    Some(ok_ty)
}

pub(super) fn cargo_crate_name(package_name: &str) -> String {
    package_name.replace('-', "_")
}

pub(super) fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{rust_function_ident, rust_ident};

    #[test]
    fn rust_function_ident_escapes_after_flattening_dotted_names() {
        assert_eq!(rust_function_ident("MultiBuffer.ref"), "MultiBuffer_ref");
        assert_eq!(rust_function_ident("gen"), "r#gen");
        assert_eq!(rust_ident("ref"), "r#ref");
    }
}
