//! Declaration-identity diagnostics owned by the semantic layer.
//!
//! These checks consume the resolved HIR inventory rather than compiler
//! orchestration state, so every frontend client can obtain the same duplicate
//! declaration facts and source spans.

use std::collections::HashSet;

use crate::hir::{DuplicateSymbolKind, Hir, HirBlock, HirExpr, HirStmt, assign_target_reads};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{Item, Program};

/// Derive stable duplicate-declaration diagnostics from a resolved HIR symbol
/// inventory. The HIR lowerer records the first and duplicate spans while it
/// constructs callable, type, constructor, and field namespaces.
pub fn duplicate_declaration_diagnostics(hir: &Hir) -> Vec<Diagnostic> {
    hir.duplicate_symbols()
        .iter()
        .map(|duplicate| {
            Diagnostic::error(
                code::DUPLICATE_DECLARATION,
                format!(
                    "{} `{}` is declared more than once.",
                    duplicate_symbol_label(duplicate.kind),
                    duplicate.name
                ),
                duplicate.duplicate_span.clone(),
                "duplicate declaration",
            )
            .with_cause(format!(
                "The first declaration is at {}:{}.",
                duplicate.first_span.line, duplicate.first_span.column
            ))
            .with_fix(
                "rename_declaration",
                "Rename or remove one declaration so the symbol table is unambiguous.",
                "manual",
            )
        })
        .collect()
}

/// Derive unresolved field-access diagnostics from resolved HIR type facts.
pub fn unknown_field_diagnostics(hir: &Hir) -> Vec<Diagnostic> {
    hir.function_bodies()
        .flat_map(|(_, body)| body.field_accesses.iter())
        .filter_map(|access| {
            let base_type = access.base_type.as_deref()?;
            let type_info = hir.type_info(base_type)?;
            (!type_info.fields.contains_key(&access.name)).then(|| {
                Diagnostic::error(
                    code::UNKNOWN_FIELD,
                    format!("unknown field `{}` on type `{base_type}`.", access.name),
                    access.span.clone(),
                    "unknown field",
                )
                .with_cause("RSScript field accesses must resolve before Rust lowering.")
                .with_fix(
                    "use_declared_field",
                    format!(
                        "Use a field declared on `{base_type}` or update the type declaration."
                    ),
                    "manual",
                )
            })
        })
        .collect()
}

/// Derive unresolved value-binding diagnostics from source-level global names
/// and resolved HIR bodies.  Lexical scope construction is a semantic fact:
/// compiler clients must not re-interpret `let`, pattern, closure, task, or
/// resource bindings while deciding whether an identifier is valid.
pub fn unknown_binding_diagnostics(hir: &Hir, source_program: &Program) -> Vec<Diagnostic> {
    let global_names = source_program
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Const(decl) => vec![decl.name.clone()],
            Item::SumType(sum) => sum
                .variants
                .iter()
                .map(|variant| variant.name.clone())
                .collect(),
            _ => Vec::new(),
        })
        .collect::<HashSet<_>>();
    let mut diagnostics = Vec::new();

    for function in source_program.items.iter().filter_map(|item| match item {
        Item::Function(function) => Some(function),
        _ => None,
    }) {
        let Some(block) = hir
            .function_body(&function.name)
            .and_then(|body| body.block.as_ref())
        else {
            continue;
        };
        let mut visible = function
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect::<HashSet<_>>();
        visible.extend(global_names.iter().cloned());
        collect_unknown_bindings_in_block(block, &mut visible, &mut diagnostics);
    }

    diagnostics
}

fn collect_unknown_bindings_in_block(
    block: &HirBlock,
    visible: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        collect_unknown_bindings_in_stmt(statement, visible, diagnostics);
    }
}

fn collect_unknown_bindings_in_stmt(
    statement: &HirStmt,
    visible: &mut HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        HirStmt::Let { name, value, .. } => {
            if let Some(value) = value {
                collect_unknown_bindings_in_expr(value, visible, diagnostics);
            }
            visible.insert(name.clone());
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_unknown_bindings_in_expr(value, visible, diagnostics);
            }
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_unknown_bindings_in_expr(resource, visible, diagnostics);
            let mut body_visible = visible.clone();
            body_visible.insert(binding.clone());
            collect_unknown_bindings_in_block(body, &mut body_visible, diagnostics);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_unknown_bindings_in_expr(condition, visible, diagnostics);
            let mut then_visible = visible.clone();
            collect_unknown_bindings_in_block(then_body, &mut then_visible, diagnostics);
            if let Some(else_body) = else_body {
                let mut else_visible = visible.clone();
                collect_unknown_bindings_in_block(else_body, &mut else_visible, diagnostics);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_unknown_bindings_in_expr(condition, visible, diagnostics);
            }
            let mut body_visible = visible.clone();
            collect_unknown_bindings_in_block(body, &mut body_visible, diagnostics);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_unknown_bindings_in_expr(iterable, visible, diagnostics);
            let mut body_visible = visible.clone();
            body_visible.insert(binding.clone());
            collect_unknown_bindings_in_block(body, &mut body_visible, diagnostics);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_unknown_bindings_in_expr(value, visible, diagnostics);
            for arm in arms {
                let mut arm_visible = visible.clone();
                arm_visible.extend(
                    arm.pattern
                        .binding_names()
                        .into_iter()
                        .map(ToString::to_string),
                );
                if let Some(guard) = &arm.guard {
                    collect_unknown_bindings_in_expr(guard, &arm_visible, diagnostics);
                }
                collect_unknown_bindings_in_block(&arm.body, &mut arm_visible, diagnostics);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_unknown_bindings_in_expr(&arm.operation, visible, diagnostics);
                let mut arm_visible = visible.clone();
                if arm.binding != "_" {
                    arm_visible.insert(arm.binding.clone());
                }
                collect_unknown_bindings_in_block(&arm.body, &mut arm_visible, diagnostics);
            }
        }
        HirStmt::Expr(value) => collect_unknown_bindings_in_expr(value, visible, diagnostics),
        HirStmt::Assign { target, value, .. } => {
            for read in assign_target_reads(target) {
                collect_unknown_bindings_in_expr(read, visible, diagnostics);
            }
            collect_unknown_bindings_in_expr(value, visible, diagnostics);
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_unknown_bindings_in_expr(
    expr: &HirExpr,
    visible: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if !visible.contains(name) && !crate::is_builtin_value_ident(name) {
                diagnostics.push(unknown_binding_diagnostic(name, span));
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_unknown_bindings_in_expr(left, visible, diagnostics);
            collect_unknown_bindings_in_expr(right, visible, diagnostics);
        }
        HirExpr::Field { base, .. } => collect_unknown_bindings_in_expr(base, visible, diagnostics),
        HirExpr::Index { base, index, .. } => {
            collect_unknown_bindings_in_expr(base, visible, diagnostics);
            collect_unknown_bindings_in_expr(index, visible, diagnostics);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_unknown_bindings_in_expr(&arg.value, visible, diagnostics);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_unknown_bindings_in_expr(value, visible, diagnostics)
        }
        HirExpr::Closure { params, body, .. } => {
            let mut closure_visible = visible.clone();
            closure_visible.extend(params.iter().cloned());
            collect_unknown_bindings_in_block(body, &mut closure_visible, diagnostics);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_unknown_bindings_in_expr(value, visible, diagnostics);
            for arm in arms {
                let mut arm_visible = visible.clone();
                arm_visible.extend(
                    arm.pattern
                        .binding_names()
                        .into_iter()
                        .map(ToString::to_string),
                );
                if let Some(guard) = &arm.guard {
                    collect_unknown_bindings_in_expr(guard, &arm_visible, diagnostics);
                }
                collect_unknown_bindings_in_block(&arm.body, &mut arm_visible, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_unknown_bindings_in_expr(&entry.key, visible, diagnostics);
                collect_unknown_bindings_in_expr(&entry.value, visible, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn unknown_binding_diagnostic(name: &str, span: &rsscript_syntax::Span) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_BINDING,
        format!("unknown value binding `{name}`."),
        span.clone(),
        "unknown binding",
    )
    .with_cause("RSScript values must resolve before Rust lowering.")
    .with_fix(
        "declare_binding",
        format!("Declare `{name}` before using it or pass it as a parameter."),
        "manual",
    )
}

fn duplicate_symbol_label(kind: DuplicateSymbolKind) -> &'static str {
    match kind {
        DuplicateSymbolKind::Function => "function",
        DuplicateSymbolKind::Type => "type",
        DuplicateSymbolKind::Constructor => "callable",
        DuplicateSymbolKind::Field => "field",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_keep_resolved_hir_identity_and_source_spans() {
        let program = rsscript_syntax::parse_source(
            "duplicate.rss",
            "fn same() -> Unit { return Unit }\nfn same() -> Unit { return Unit }\n",
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = duplicate_declaration_diagnostics(&hir);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::DUPLICATE_DECLARATION);
        assert_eq!(diagnostics[0].span.file, "duplicate.rss");
        assert_eq!(diagnostics[0].span.line, 2);
    }

    #[test]
    fn unknown_bindings_follow_hir_lexical_scope_and_keep_source_spans() {
        let program = rsscript_syntax::parse_source(
            "bindings.rss",
            r#"
const global = 1
sum Choice { Some(value: Int) }
fn check(param: Int) -> Unit {
    let local = param
    match Some(local) {
        Some(bound) => { let inner = bound }
    }
    missing
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = unknown_binding_diagnostics(&hir, &program);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::UNKNOWN_BINDING);
        assert_eq!(diagnostics[0].span.file, "bindings.rss");
        assert_eq!(diagnostics[0].span.line, 9);
    }
}
