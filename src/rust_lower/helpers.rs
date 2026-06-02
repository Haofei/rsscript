use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::{Diagnostic, Span, code};
use crate::interfaces::builtin_interfaces;
use crate::runtime_abi;
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FileFeature, FunctionDecl, GenericBound,
    GenericParam, Item, MatchLiteral, MatchPattern, Param, Program, Stmt, TypeRef,
};
use crate::syntax::parse_source;

use super::lowerer::RustLowerer;

pub(super) fn validate_executable_declarations(
    program: &Program,
    native_bindings: &BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    let mut implemented = BTreeSet::new();
    let mut bodyless = BTreeMap::new();
    let mut native_bodyless = BTreeMap::new();
    let protocol_method_keys = protocol_method_keys(program);
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let key = executable_declaration_function_key(&function.name);
        if protocol_method_keys.contains(&key) {
            continue;
        }
        if function.body.statements.is_empty() {
            if function.is_native {
                native_bodyless.insert(key, function.name.clone());
            } else {
                bodyless.insert(key, function.name.clone());
            }
        } else {
            implemented.insert(key);
        }
    }
    for key in &implemented {
        bodyless.remove(key);
        native_bodyless.remove(key);
    }
    if bodyless.is_empty() && native_bodyless.is_empty() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    let context = ExecutableDeclarationValidation {
        bodyless: &bodyless,
        native_bodyless: &native_bodyless,
        native_bindings,
    };
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        if function.body.statements.is_empty() {
            continue;
        }
        validate_executable_declarations_in_block(&function.body, &context, &mut diagnostics);
    }
    diagnostics
}

struct ExecutableDeclarationValidation<'a> {
    bodyless: &'a BTreeMap<String, String>,
    native_bodyless: &'a BTreeMap<String, String>,
    native_bindings: &'a BTreeMap<String, String>,
}

fn validate_executable_declarations_in_block(
    block: &Block,
    context: &ExecutableDeclarationValidation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        validate_executable_declarations_in_stmt(statement, context, diagnostics);
    }
}

fn validate_executable_declarations_in_stmt(
    statement: &Stmt,
    context: &ExecutableDeclarationValidation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                validate_executable_declarations_in_expr(value, context, diagnostics);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                validate_executable_declarations_in_expr(value, context, diagnostics);
            }
        }
        Stmt::With(stmt) => {
            validate_executable_declarations_in_expr(&stmt.resource, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::If(stmt) => {
            validate_executable_declarations_in_expr(&stmt.condition, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.then_body, context, diagnostics);
            if let Some(else_body) = &stmt.else_body {
                validate_executable_declarations_in_block(else_body, context, diagnostics);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                validate_executable_declarations_in_expr(condition, context, diagnostics);
            }
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::For(stmt) => {
            validate_executable_declarations_in_expr(&stmt.iterable, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::TaskGroup(stmt) => {
            validate_executable_declarations_in_block(&stmt.body, context, diagnostics);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                validate_executable_declarations_in_expr(&arm.operation, context, diagnostics);
                validate_executable_declarations_in_block(&arm.body, context, diagnostics);
            }
        }
        Stmt::Match(stmt) => {
            validate_executable_declarations_in_expr(&stmt.value, context, diagnostics);
            for arm in &stmt.arms {
                validate_executable_declarations_in_block(&arm.body, context, diagnostics);
            }
        }
        Stmt::LetElse(stmt) => {
            validate_executable_declarations_in_expr(&stmt.value, context, diagnostics);
            validate_executable_declarations_in_block(&stmt.else_body, context, diagnostics);
        }
        Stmt::Assign(stmt) => {
            validate_executable_declarations_in_expr(&stmt.target, context, diagnostics);
            validate_executable_declarations_in_expr(&stmt.value, context, diagnostics);
        }
        Stmt::Expr(expr) => validate_executable_declarations_in_expr(expr, context, diagnostics),
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

fn validate_executable_declarations_in_expr(
    expr: &Expr,
    context: &ExecutableDeclarationValidation<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            let key = executable_declaration_callee_key(callee);
            if runtime_intrinsic_target(callee).is_none()
                && let Some(function_name) = context.bodyless.get(&key)
            {
                diagnostics.push(
                    Diagnostic::error(
                        code::UNSUPPORTED_SYNTAX,
                        "unsupported executable RSScript declaration call.",
                        span.clone(),
                        "unimplemented declaration call",
                    )
                    .with_cause(format!(
                        "`{function_name}` is a declaration without a RSScript body. Provide an implementation or bind it as a native/runtime intrinsic before executable lowering."
                    )),
                );
            }
            if runtime_intrinsic_target(callee).is_none()
                && !context.native_bindings.contains_key(&key)
                && let Some(function_name) = context.native_bodyless.get(&key)
            {
                diagnostics.push(
                    Diagnostic::error(
                        code::UNSUPPORTED_SYNTAX,
                        "unbound native RSScript declaration call.",
                        span.clone(),
                        "unbound native declaration call",
                    )
                    .with_cause(format!(
                        "`{function_name}` is a native declaration without a configured Rust binding. Add a native binding before executable lowering."
                    )),
                );
            }
            for arg in args {
                validate_executable_declarations_in_expr(&arg.value, context, diagnostics);
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_executable_declarations_in_expr(left, context, diagnostics);
            validate_executable_declarations_in_expr(right, context, diagnostics);
        }
        Expr::Field { base, .. } => {
            validate_executable_declarations_in_expr(base, context, diagnostics);
        }
        Expr::Index { base, index, .. } => {
            validate_executable_declarations_in_expr(base, context, diagnostics);
            validate_executable_declarations_in_expr(index, context, diagnostics);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            validate_executable_declarations_in_expr(value, context, diagnostics);
        }
        Expr::Closure { body, .. } => {
            validate_executable_declarations_in_block(body, context, diagnostics);
        }
        Expr::Match { value, arms, .. } => {
            validate_executable_declarations_in_expr(value, context, diagnostics);
            for arm in arms {
                validate_executable_declarations_in_block(&arm.body, context, diagnostics);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::MapLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

pub(super) fn executable_declaration_function_key(name: &str) -> String {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        format!("{}.{}", type_root_name(namespace), name)
    } else {
        name.to_string()
    }
}

pub(super) fn executable_declaration_callee_key(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => type_root_name(name).to_string(),
        Callee::Qualified { namespace, name } => {
            format!("{}.{}", type_root_name(namespace), type_root_name(name))
        }
        Callee::ReceiverCall { method, .. } => type_root_name(method).to_string(),
    }
}

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

pub(super) fn collect_function_return_types(program: &Program) -> BTreeMap<String, TypeRef> {
    let mut return_types = BTreeMap::new();
    collect_program_function_return_types(program, &mut return_types);
    for (file, source) in builtin_interfaces() {
        let interface_program = parse_source(file, source);
        collect_program_function_return_types(&interface_program, &mut return_types);
    }
    return_types
}

pub(super) fn collect_function_param_types(
    program: &Program,
) -> BTreeMap<String, Vec<(String, TypeRef)>> {
    let mut param_types = BTreeMap::new();
    for (file, source) in builtin_interfaces() {
        let interface_program = parse_source(file, source);
        collect_program_function_param_types(&interface_program, &mut param_types);
    }
    collect_program_function_param_types(program, &mut param_types);
    param_types
}

pub(super) fn collect_function_param_effects(
    program: &Program,
) -> BTreeMap<String, Vec<(String, Option<DataEffect>)>> {
    let mut param_effects = BTreeMap::new();
    for (file, source) in builtin_interfaces() {
        let interface_program = parse_source(file, source);
        collect_program_function_param_effects(&interface_program, &mut param_effects);
    }
    collect_program_function_param_effects(program, &mut param_effects);
    param_effects
}

fn collect_program_function_param_effects(
    program: &Program,
    param_effects: &mut BTreeMap<String, Vec<(String, Option<DataEffect>)>>,
) {
    for item in &program.items {
        if let Item::Function(function) = item {
            param_effects.insert(
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.effect))
                    .collect(),
            );
        }
    }
}

fn collect_program_function_param_types(
    program: &Program,
    param_types: &mut BTreeMap<String, Vec<(String, TypeRef)>>,
) {
    for item in &program.items {
        if let Item::Function(function) = item {
            param_types.insert(
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.ty.clone()))
                    .collect(),
            );
        }
    }
}

pub(super) fn collect_function_retained_params(
    program: &Program,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut retained_params = BTreeMap::new();
    for (file, source) in builtin_interfaces() {
        let interface_program = parse_source(file, source);
        collect_program_function_retained_params(&interface_program, &mut retained_params);
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

pub(super) fn collect_native_boundary_callees(program: &Program) -> BTreeSet<String> {
    let mut callees = BTreeSet::new();
    for (file, source) in builtin_interfaces() {
        let interface = parse_source(file, source);
        collect_native_boundary_callees_from_program(&interface, &mut callees);
    }
    collect_native_boundary_callees_from_program(program, &mut callees);
    callees
}

pub(super) fn collect_async_native_boundary_callees(program: &Program) -> BTreeSet<String> {
    let mut callees = BTreeSet::new();
    for (file, source) in builtin_interfaces() {
        let interface = parse_source(file, source);
        collect_async_native_boundary_callees_from_program(&interface, &mut callees);
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
                && *effect == DataEffect::Mut
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
        MatchPattern::Wildcard(_) => "_".to_string(),
        MatchPattern::Literal { value, .. } => lower_match_literal(value),
        MatchPattern::Variant {
            name,
            binding: Some(binding),
            ..
        } => format!("{}({})", rust_ident(name), rust_ident(binding)),
        MatchPattern::Variant { name, .. } if matches!(name.as_str(), "Some" | "Ok" | "Err") => {
            format!("{}(_)", rust_ident(name))
        }
        MatchPattern::Variant { name, .. } => rust_ident(name),
    }
}

pub(super) fn lower_match_pattern_qualified(pattern: &MatchPattern, sum_type: &str) -> String {
    match pattern {
        MatchPattern::Wildcard(_) => "_".to_string(),
        MatchPattern::Literal { value, .. } => lower_match_literal(value),
        MatchPattern::Variant {
            name,
            binding: Some(binding),
            ..
        } => format!(
            "{}::{} {{ {}: {}, .. }}",
            rust_ident(sum_type),
            rust_ident(name),
            rust_ident(binding),
            rust_ident(binding)
        ),
        MatchPattern::Variant { name, .. } => {
            format!("{}::{}", rust_ident(sum_type), rust_ident(name))
        }
    }
}

fn lower_match_literal(value: &MatchLiteral) -> String {
    match value {
        MatchLiteral::Int(value) => value.clone(),
        MatchLiteral::String(value) => format!("{:?}", decode_string_token(value)),
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
                None => name,
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
        Callee::Name(name) => rust_ident(type_root_name(name)),
        Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" => {
            format!(
                "rsscript_runtime::ResourcePool::{}",
                rust_ident(type_root_name(name))
            )
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
        return match param.effect {
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

pub(super) fn runtime_intrinsic_target(callee: &Callee) -> Option<&'static str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    runtime_abi::lookup_runtime_intrinsic(type_root_name(namespace), type_root_name(name))
        .map(|intrinsic| intrinsic.rust_target)
}

pub(super) fn runtime_intrinsic_wants_managed_handle_arg(
    callee: &Callee,
    arg_name: Option<&str>,
) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    let Some(arg_name) = arg_name else {
        return false;
    };
    runtime_abi::lookup_runtime_intrinsic(type_root_name(namespace), type_root_name(name))
        .is_some_and(|intrinsic| intrinsic.managed_handle_args.contains(&arg_name))
}

pub(super) fn is_string_concat_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "String" && type_root_name(name) == "concat")
}

pub(super) fn is_file_open_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && type_root_name(name) == "open")
}

pub(super) fn is_async_runtime_intrinsic_callee(callee: &Callee) -> bool {
    let Callee::Qualified { namespace, name } = callee else {
        return false;
    };
    let (namespace, name) = (type_root_name(namespace), type_root_name(name));
    matches!(
        (namespace, name),
        ("Timer", "sleep" | "sleep_until" | "sleep_cancellable")
            | (
                "File",
                "read_all_async" | "read_all_string_async" | "write_async" | "write_string_async",
            )
            | ("Http", "get_async" | "post_form_async" | "post_json_async")
            | ("Sender", "send" | "send_cancellable")
            | ("Receiver", "recv" | "recv_cancellable")
            | ("Stream", "next")
    )
}

pub(super) fn is_file_open_read_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && type_root_name(name) == "open_read")
}

pub(super) fn is_file_open_write_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "File" && type_root_name(name) == "open_write")
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

pub(super) fn is_resource_pool_borrow_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "borrow")
}

pub(super) fn is_resource_pool_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "new")
}

pub(super) fn is_resource_pool_try_new_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_new")
}

pub(super) fn is_resource_pool_lazy_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "lazy")
}

pub(super) fn is_resource_pool_try_lazy_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_lazy")
}

pub(super) fn is_resource_pool_try_borrow_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && type_root_name(name) == "try_borrow")
}

pub(super) fn is_resource_pool_borrow_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Call { callee, .. } if is_resource_pool_borrow_callee(callee))
}

pub(super) fn lower_required_call_arg(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    name: &str,
    index: usize,
    call_span: &Span,
) -> String {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .or_else(|| args.get(index))
        .map(|arg| lowerer.lower_expr(&arg.value))
        .unwrap_or_else(|| unreachable_lowering("validated call argument", call_span))
}

pub(super) fn lower_resource_pool_new_call(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_required_call_arg(lowerer, args, "create", 0, call_span);
    let max_size = lower_required_call_arg(lowerer, args, "max_size", 1, call_span);
    format!("rsscript_runtime::ResourcePool::from_factory({max_size}, {create})")
}

pub(super) fn lower_resource_pool_try_new_call(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_required_call_arg(lowerer, args, "create", 0, call_span);
    let max_size = lower_required_call_arg(lowerer, args, "max_size", 1, call_span);
    format!("rsscript_runtime::ResourcePool::try_from_factory({max_size}, {create})")
}

pub(super) fn lower_resource_pool_lazy_call(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_resource_pool_owning_factory(lowerer, args, call_span);
    let max_size = lower_required_call_arg(lowerer, args, "max_size", 1, call_span);
    format!("rsscript_runtime::ResourcePool::lazy_from_factory({max_size}, {create})")
}

pub(super) fn lower_resource_pool_try_lazy_call(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_resource_pool_owning_factory(lowerer, args, call_span);
    let max_size = lower_required_call_arg(lowerer, args, "max_size", 1, call_span);
    format!("rsscript_runtime::ResourcePool::try_lazy_from_factory({max_size}, {create})")
}

/// A lazy factory is stored in the pool and must own its captures (`'static`), so
/// its closure is lowered with `move`. The eager factories run immediately and
/// keep borrowing semantics.
fn lower_resource_pool_owning_factory(
    lowerer: &mut RustLowerer<'_>,
    args: &[CallArg],
    call_span: &Span,
) -> String {
    let create = lower_required_call_arg(lowerer, args, "create", 0, call_span);
    if create.trim_start().starts_with('|') {
        format!("move {create}")
    } else {
        create
    }
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

pub(super) fn decode_string_token(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some('0') => decoded.push('\0'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

pub(super) fn is_rust_enum_constructor(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some")
}

pub(super) fn lower_source_span(span: &Span) -> String {
    format!(
        "rsscript_runtime::SourceSpan::new({:?}, {}, {}, {})",
        span.file, span.line, span.column, span.length
    )
}

pub(super) fn rust_function_ident(name: &str) -> String {
    name.split('.')
        .map(rust_ident)
        .collect::<Vec<_>>()
        .join("_")
}

pub(super) fn rust_qualified_function_ident(namespace: &str, name: &str) -> String {
    namespace
        .split('.')
        .chain(std::iter::once(type_root_name(name)))
        .map(rust_path_segment)
        .collect::<Vec<_>>()
        .join("_")
}

pub(super) fn type_root_name(name: &str) -> &str {
    let name = name.trim().strip_prefix("fresh ").unwrap_or(name.trim());
    name.split('<').next().unwrap_or(name)
}

pub(super) fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let inner = type_name
        .split_once('<')
        .and_then(|(_, rest)| rest.strip_suffix('>'))?;
    Some(split_top_level_type_args(inner))
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

pub(super) fn rust_path_segment(segment: &str) -> String {
    if let Some((head, tail)) = segment.split_once("::<") {
        format!("{}::<{}", rust_ident(head), tail)
    } else {
        rust_ident(segment)
    }
}

pub(super) fn rust_ident(name: &str) -> String {
    let keywords: BTreeSet<&'static str> = [
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ]
    .into_iter()
    .collect();

    if keywords.contains(name) {
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
        RunnableMainKind::ResultUnit => format!(
            "{}::{}().expect(\"RSScript main returned an error\");",
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
