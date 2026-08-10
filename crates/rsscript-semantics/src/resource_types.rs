//! Platform-neutral resource declaration diagnostics.

use crate::hir::{Hir, HirTypeKind};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{Block, Callee, Expr, Item, Program, Stmt, TypeKind, TypeRef};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResourceGenericContext {
    Ordinary,
    Return,
}

/// Reject raw descriptor handles outside resource-internal declarations.
pub fn fd_surface_diagnostics(program: &Program) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        match item {
            Item::Function(function) => {
                for param in &function.params {
                    if type_ref_contains_name(&param.ty, "Fd") {
                        diagnostics.push(fd_surface_diagnostic(
                            &param.ty.span,
                            "`Fd` parameter outside native boundary",
                            "Use a `resource` type such as `File` instead of exposing raw descriptor handles.",
                        ));
                    }
                }
                if let Some(return_ty) = &function.return_ty
                    && type_ref_contains_name(return_ty, "Fd")
                {
                    diagnostics.push(fd_surface_diagnostic(
                        &return_ty.span,
                        "`Fd` return outside native boundary",
                        "Return a `resource` type such as `File` instead of exposing raw descriptor handles.",
                    ));
                }
            }
            Item::Type(decl) if decl.kind != TypeKind::Resource => {
                for field in &decl.fields {
                    if type_ref_contains_name(&field.ty, "Fd") {
                        diagnostics.push(fd_surface_diagnostic(
                            &field.ty.span,
                            "`Fd` field outside resource internals",
                            "Use a `resource` field wrapper or a non-Fd public value type.",
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    diagnostics
}

/// Reject resource fields stored in non-resource declarations.
pub fn resource_field_diagnostics(hir: &Hir, program: &Program) -> Vec<Diagnostic> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Type(decl) if hir.type_kind(&decl.name) != Some(HirTypeKind::Resource) => {
                Some((decl.name.as_str(), decl.fields.as_slice()))
            }
            _ => None,
        })
        .flat_map(|(container, fields)| {
            fields
                .iter()
                .filter(move |field| hir.type_kind(&field.ty.name) == Some(HirTypeKind::Resource))
                .map(move |field| {
                    Diagnostic::error(
                        code::RESOURCE_FIELD,
                        format!(
                            "resource `{}` cannot be stored in `{container}`.",
                            field.ty.name
                        ),
                        field.span.clone(),
                        "resource field",
                    )
                    .with_cause("Resources must be used through `with`.")
                    .with_fix(
                        "use_with",
                        "Use the resource through `with` instead.",
                        "manual",
                    )
                })
        })
        .collect()
}

/// Ensure weak fields point at identity-managed classes.
pub fn weak_field_diagnostics(hir: &Hir, program: &Program) -> Vec<Diagnostic> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Type(decl) => Some(decl.fields.as_slice()),
            _ => None,
        })
        .flat_map(|fields| {
            fields
                .iter()
                .filter(|field| {
                    field.is_weak && hir.type_kind(&field.ty.name) != Some(HirTypeKind::Class)
                })
                .map(|field| {
                    Diagnostic::error(
                        code::INVALID_WEAK_FIELD,
                        format!(
                            "weak field `{}` must point to a class, but `{}` is not a class.",
                            field.name, field.ty.name
                        ),
                        field.span.clone(),
                        "invalid weak field",
                    )
                    .with_cause(
                        "`weak` is only for breaking managed identity-object cycles in the MVP.",
                    )
                    .with_fix(
                        "use_class_or_remove_weak",
                        "Use a class type for the weak field, or remove `weak`.",
                        "manual",
                    )
                })
        })
        .collect()
}

/// Reject resource generic containment in declarations and explicit generic
/// call namespaces. `Result<Resource, E>` remains valid only in a direct
/// function return position.
pub fn resource_generic_diagnostics(hir: &Hir, program: &Program) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) => {
                for field in &decl.fields {
                    collect_resource_generic_type_ref(
                        hir,
                        &field.ty,
                        ResourceGenericContext::Ordinary,
                        &mut diagnostics,
                    );
                }
            }
            Item::Function(function) => {
                for param in &function.params {
                    collect_resource_generic_type_ref(
                        hir,
                        &param.ty,
                        ResourceGenericContext::Ordinary,
                        &mut diagnostics,
                    );
                }
                if let Some(return_ty) = &function.return_ty {
                    collect_resource_generic_type_ref(
                        hir,
                        return_ty,
                        ResourceGenericContext::Return,
                        &mut diagnostics,
                    );
                }
                collect_resource_generic_calls_in_block(hir, &function.body, &mut diagnostics);
            }
            Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => {}
        }
    }
    diagnostics
}

fn collect_resource_generic_type_ref(
    hir: &Hir,
    ty: &TypeRef,
    context: ResourceGenericContext,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (index, arg) in ty.args.iter().enumerate() {
        if hir.type_kind(&arg.name) == Some(HirTypeKind::Resource)
            && !resource_result_return_arg_allowed(ty, index, context)
        {
            diagnostics.push(resource_generic_argument_diagnostic(
                &ty.name, &arg.name, &arg.span,
            ));
        }
    }
    for (index, arg) in ty.args.iter().enumerate() {
        if !resource_result_return_arg_allowed(ty, index, context) {
            collect_resource_generic_type_ref(
                hir,
                arg,
                ResourceGenericContext::Ordinary,
                diagnostics,
            );
        }
    }
}

fn collect_resource_generic_calls_in_block(
    hir: &Hir,
    block: &Block,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        collect_resource_generic_calls_in_stmt(hir, statement, diagnostics);
    }
}

fn collect_resource_generic_calls_in_stmt(
    hir: &Hir,
    statement: &Stmt,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_resource_generic_calls_in_expr(hir, value, diagnostics);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_resource_generic_calls_in_expr(hir, value, diagnostics);
            }
        }
        Stmt::Assign(stmt) => {
            collect_resource_generic_calls_in_expr(hir, &stmt.target, diagnostics);
            collect_resource_generic_calls_in_expr(hir, &stmt.value, diagnostics);
        }
        Stmt::Expr(value) => collect_resource_generic_calls_in_expr(hir, value, diagnostics),
        Stmt::With(stmt) => {
            collect_resource_generic_calls_in_expr(hir, &stmt.resource, diagnostics);
            collect_resource_generic_calls_in_block(hir, &stmt.body, diagnostics);
        }
        Stmt::If(stmt) => {
            collect_resource_generic_calls_in_expr(hir, &stmt.condition, diagnostics);
            collect_resource_generic_calls_in_block(hir, &stmt.then_body, diagnostics);
            if let Some(else_body) = &stmt.else_body {
                collect_resource_generic_calls_in_block(hir, else_body, diagnostics);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_resource_generic_calls_in_expr(hir, condition, diagnostics);
            }
            collect_resource_generic_calls_in_block(hir, &stmt.body, diagnostics);
        }
        Stmt::For(stmt) => {
            collect_resource_generic_calls_in_expr(hir, &stmt.iterable, diagnostics);
            collect_resource_generic_calls_in_block(hir, &stmt.body, diagnostics);
        }
        Stmt::TaskGroup(stmt) => {
            collect_resource_generic_calls_in_block(hir, &stmt.body, diagnostics)
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_resource_generic_calls_in_expr(hir, &arm.operation, diagnostics);
                collect_resource_generic_calls_in_block(hir, &arm.body, diagnostics);
            }
        }
        Stmt::Match(stmt) => {
            collect_resource_generic_calls_in_expr(hir, &stmt.value, diagnostics);
            for arm in &stmt.arms {
                collect_resource_generic_calls_in_block(hir, &arm.body, diagnostics);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_resource_generic_calls_in_expr(hir, &stmt.value, diagnostics);
            collect_resource_generic_calls_in_block(hir, &stmt.else_body, diagnostics);
        }
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

fn collect_resource_generic_calls_in_expr(
    hir: &Hir,
    expr: &Expr,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            if let Callee::Qualified { namespace, .. } = callee
                && let Some((root, args)) = generic_namespace_args(namespace)
            {
                for arg in args {
                    if hir.type_kind(arg) == Some(HirTypeKind::Resource) {
                        diagnostics.push(resource_generic_argument_diagnostic(root, arg, span));
                    }
                }
            }
            for arg in args {
                collect_resource_generic_calls_in_expr(hir, &arg.value, diagnostics);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_resource_generic_calls_in_expr(hir, left, diagnostics);
            collect_resource_generic_calls_in_expr(hir, right, diagnostics);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            collect_resource_generic_calls_in_expr(hir, value, diagnostics)
        }
        Expr::Field { base, .. } => collect_resource_generic_calls_in_expr(hir, base, diagnostics),
        Expr::Index { base, index, .. } => {
            collect_resource_generic_calls_in_expr(hir, base, diagnostics);
            collect_resource_generic_calls_in_expr(hir, index, diagnostics);
        }
        Expr::Closure { body, .. } => {
            collect_resource_generic_calls_in_block(hir, body, diagnostics)
        }
        Expr::Match { value, arms, .. } => {
            collect_resource_generic_calls_in_expr(hir, value, diagnostics);
            for arm in arms {
                collect_resource_generic_calls_in_block(hir, &arm.body, diagnostics);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_resource_generic_calls_in_expr(hir, &entry.key, diagnostics);
                collect_resource_generic_calls_in_expr(hir, &entry.value, diagnostics);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn resource_result_return_arg_allowed(
    ty: &TypeRef,
    index: usize,
    context: ResourceGenericContext,
) -> bool {
    context == ResourceGenericContext::Return && ty.name == "Result" && index == 0
}

fn generic_namespace_args(namespace: &str) -> Option<(&str, Vec<&str>)> {
    let (root, _) = namespace.split_once('<')?;
    Some((root, crate::type_arg_names(namespace)?))
}

fn resource_generic_argument_diagnostic(
    generic_name: &str,
    resource_name: &str,
    span: &rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_GENERIC_ARGUMENT,
        format!(
            "generic type `{generic_name}` cannot be instantiated with resource `{resource_name}`."
        ),
        span.clone(),
        "resource generic argument",
    )
    .with_cause("Generic containers cannot hold resource values.")
    .with_fix(
        "use_resource_api",
        "Use the resource through `with`, or use a non-resource value type.",
        "manual",
    )
}

fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name
        || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
        || ty
            .fn_params
            .iter()
            .any(|param| type_ref_contains_name(param, name))
        || ty
            .fn_return
            .as_deref()
            .is_some_and(|return_ty| type_ref_contains_name(return_ty, name))
}

fn fd_surface_diagnostic(span: &rsscript_syntax::Span, summary: &str, fix: &str) -> Diagnostic {
    Diagnostic::error(
        code::FD_OUTSIDE_INTERNAL_BOUNDARY,
        summary,
        span.clone(),
        "Fd outside native/resource internals",
    )
    .with_cause(
        "`Fd` is a trusted native/resource-internal descriptor handle, not an ordinary RSScript value type.",
    )
    .with_fix("use_resource_wrapper", fix, "manual")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_resource_boundary_diagnostics_from_source_and_hir() {
        let program = rsscript_syntax::parse_source(
            "resources.rss",
            r#"
resource File { raw: Fd }
struct State {
    file: File
    link: weak State
    raw: Fd
}
fn expose(raw: Fd) -> Fd { raw }
"#,
        );
        let hir = Hir::from_syntax(&program);

        assert_eq!(fd_surface_diagnostics(&program).len(), 3);
        assert_eq!(resource_field_diagnostics(&hir, &program).len(), 1);
        assert_eq!(weak_field_diagnostics(&hir, &program).len(), 1);
    }

    #[test]
    fn resource_generics_reject_containment_but_allow_direct_result_returns() {
        let program = rsscript_syntax::parse_source(
            "resource-generics.rss",
            r#"
resource File { raw: Int }
struct Archive {
    files: List<File>
    backup: Option<File>
}
fn open() -> Result<File, IOError>
"#,
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = resource_generic_diagnostics(&hir, &program);

        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == code::RESOURCE_GENERIC_ARGUMENT)
        );
    }
}
