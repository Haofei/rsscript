use crate::text_util::{split_top_level_type_args, type_arg_names, type_root_name};
use std::collections::{HashMap, HashSet};

use crate::checks;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{
    CallResolution, DuplicateSymbolKind, FieldInfo, FunctionSig, Hir, HirBlock, HirExpr,
    HirMatchArm, HirStmt, HirTypeKind, ParamSig, ResolvedCalleeKind,
};
use crate::interfaces::CORE_INTERFACES;
use crate::lexer::{Token, lex};
use crate::syntax::ast::merge_programs;
use crate::syntax::ast::{
    AssignStmt, Block, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FunctionDecl, GenericBound,
    GenericParam, Item, MatchPattern, Param, Stmt, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

mod assign;
mod derives;
mod diagnostics;
mod duplicate_decls;
mod exhaustiveness;
mod resource_types;
mod runtime_guarantee;
mod signatures;
mod syntax_support;
mod unknowns;
use assign::AssignChecker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceGenericContext {
    Ordinary,
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternWitness {
    Any,
    Bool(bool),
    Constructor {
        name: String,
        fields: Vec<(String, PatternWitness)>,
    },
}

fn resource_result_return_arg_allowed(
    ty: &TypeRef,
    index: usize,
    context: ResourceGenericContext,
) -> bool {
    context == ResourceGenericContext::Return && ty.name == "Result" && index == 0
}

pub fn analyze_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let mut syntax_program = parse_source(file, source);
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let hir = Hir::from_syntax_with_standard_package_interfaces(&syntax_program);
    analyze_program(tokens, syntax_program, hir, builtin_interface_programs())
}

pub fn analyze_syntax_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let mut syntax_program = parse_source(file, source);
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let hir = Hir::from_syntax(&syntax_program);
    analyze_syntax_program(tokens, syntax_program, hir)
}

pub fn analyze_source_without_core(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let mut syntax_program = parse_source(file, source);
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let hir = Hir::from_syntax_without_builtin_interfaces(&syntax_program);
    analyze_program(tokens, syntax_program, hir, Vec::new())
}

pub fn core_interfaces() -> &'static [(&'static str, &'static str)] {
    CORE_INTERFACES
}

pub fn standard_package_interfaces() -> &'static [(&'static str, &'static str)] {
    crate::interfaces::STANDARD_PACKAGE_INTERFACES
}

pub fn analyze_source_with_core(file: &str, source: &str) -> Vec<Diagnostic> {
    analyze_source(file, source)
}

pub fn analyze_source_with_interfaces(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let mut syntax_program = parse_source(file, source);
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&syntax_program, &interface_programs);
    analyze_program(tokens, syntax_program, hir, interface_programs)
}

pub fn analyze_source_with_interfaces_without_core(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let mut syntax_program = parse_source(file, source);
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces_without_builtin_interfaces(
        &syntax_program,
        &interface_programs,
    );
    analyze_program(tokens, syntax_program, hir, interface_programs)
}

pub fn analyze_sources_with_interfaces(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let tokens = sources
        .iter()
        .flat_map(|(file, source)| lex(file, source))
        .collect::<Vec<_>>();
    let mut syntax_program = merge_programs(
        sources
            .iter()
            .map(|(file, source)| parse_source(file, source)),
    );
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&syntax_program, &interface_programs);
    analyze_program(tokens, syntax_program, hir, interface_programs)
}

pub fn analyze_sources_with_interfaces_without_core(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let tokens = sources
        .iter()
        .flat_map(|(file, source)| lex(file, source))
        .collect::<Vec<_>>();
    let mut syntax_program = merge_programs(
        sources
            .iter()
            .map(|(file, source)| parse_source(file, source)),
    );
    crate::syntax::isolate_module_namespaces(&mut syntax_program);
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces_without_builtin_interfaces(
        &syntax_program,
        &interface_programs,
    );
    analyze_program(tokens, syntax_program, hir, interface_programs)
}

fn builtin_interface_programs() -> Vec<crate::syntax::ast::Program> {
    crate::interfaces::default_interfaces()
        .map(|(file, source)| parse_source(file, source))
        .collect()
}

fn analyze_program(
    tokens: Vec<Token>,
    syntax_program: crate::syntax::ast::Program,
    hir: Hir,
    interface_programs: Vec<crate::syntax::ast::Program>,
) -> Vec<Diagnostic> {
    let mut type_aliases = std::collections::BTreeMap::new();
    for interface in builtin_interface_programs()
        .iter()
        .chain(interface_programs.iter())
    {
        type_aliases.extend(type_aliases_from_program(interface));
    }
    type_aliases.extend(type_aliases_from_program(&syntax_program));
    let mut type_alias_params = std::collections::BTreeMap::new();
    for interface in builtin_interface_programs()
        .iter()
        .chain(interface_programs.iter())
    {
        type_alias_params.extend(type_alias_params_from_program(interface));
    }
    type_alias_params.extend(type_alias_params_from_program(&syntax_program));
    let mut analyzer = Analyzer {
        tokens: &tokens,
        syntax_program,
        interface_programs,
        hir,
        diagnostics: Vec::new(),
        type_aliases,
        type_alias_params,
        in_task_group: false,
        async_let_names: Vec::new(),
    };
    analyzer.run();
    let mut diagnostics = analyzer.diagnostics;
    crate::syntax::demangle_diagnostics(&analyzer.syntax_program, &mut diagnostics);
    diagnostics
}

/// Namespaces that the compiler generates and a user declaration must not claim:
/// desugaring temporaries (`__rss_*`) and runtime helpers (`__rsscript_*`). Other
/// `__`-prefixed names (Python-style dunders like `__hash__`, `__eq__`, and the
/// synthetic `__TupleN` structs the tuple desugar injects) are legal — they don't
/// collide with any generated namespace.
fn is_reserved_generated_name(leaf: &str) -> bool {
    leaf.starts_with("__rss_") || leaf.starts_with("__rsscript_")
}

fn type_aliases_from_program(
    program: &crate::syntax::ast::Program,
) -> impl Iterator<Item = (String, String)> + '_ {
    use crate::syntax::ast::Item;
    program.items.iter().filter_map(|item| {
        if let Item::TypeAlias(alias) = item {
            Some((alias.name.clone(), type_ref_display_name(&alias.target)))
        } else {
            None
        }
    })
}

/// The generic parameter names of each type alias (`type Pair<T> = ...` → `T`),
/// so generic aliases can be expanded by substituting arguments for parameters.
fn type_alias_params_from_program(
    program: &crate::syntax::ast::Program,
) -> impl Iterator<Item = (String, Vec<String>)> + '_ {
    use crate::syntax::ast::Item;
    program.items.iter().filter_map(|item| {
        if let Item::TypeAlias(alias) = item {
            Some((
                alias.name.clone(),
                alias
                    .type_params
                    .iter()
                    .map(|param| param.name.clone())
                    .collect(),
            ))
        } else {
            None
        }
    })
}

fn analyze_syntax_program(
    tokens: Vec<Token>,
    syntax_program: crate::syntax::ast::Program,
    hir: Hir,
) -> Vec<Diagnostic> {
    let mut analyzer = Analyzer {
        tokens: &tokens,
        syntax_program,
        interface_programs: Vec::new(),
        hir,
        diagnostics: Vec::new(),
        type_aliases: Default::default(),
        type_alias_params: Default::default(),
        in_task_group: false,
        async_let_names: Vec::new(),
    };
    analyzer.run_syntax_only();
    let mut diagnostics = analyzer.diagnostics;
    crate::syntax::demangle_diagnostics(&analyzer.syntax_program, &mut diagnostics);
    diagnostics
}

fn type_ref_display_name(ty: &crate::syntax::ast::TypeRef) -> String {
    if ty.args.is_empty() {
        ty.name.clone()
    } else {
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
}

pub(crate) struct Analyzer<'a> {
    pub(crate) tokens: &'a [Token],
    pub(crate) syntax_program: crate::syntax::ast::Program,
    interface_programs: Vec<crate::syntax::ast::Program>,
    pub(crate) hir: Hir,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) type_aliases: std::collections::BTreeMap<String, String>,
    /// Type-alias name -> its generic parameter names (empty for non-generic
    /// aliases), used to expand generic aliases like `Pair<Int>`.
    pub(crate) type_alias_params: std::collections::BTreeMap<String, Vec<String>>,
    in_task_group: bool,
    pub(crate) async_let_names: Vec<String>,
}

fn collect_task_group_async_lets(
    block: &Block,
    async_lets: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.is_async {
                    async_lets.push((stmt.name.clone(), stmt.span.clone()));
                }
                if let Some(value) = &stmt.value {
                    collect_task_group_async_lets_expr(value, async_lets);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_async_lets_expr(value, async_lets);
                }
            }
            Stmt::With(stmt) => {
                collect_task_group_async_lets_expr(&stmt.resource, async_lets);
                collect_task_group_async_lets(&stmt.body, async_lets);
            }
            Stmt::If(stmt) => {
                collect_task_group_async_lets_expr(&stmt.condition, async_lets);
                collect_task_group_async_lets(&stmt.then_body, async_lets);
                if let Some(else_body) = &stmt.else_body {
                    collect_task_group_async_lets(else_body, async_lets);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_task_group_async_lets_expr(condition, async_lets);
                }
                collect_task_group_async_lets(&stmt.body, async_lets);
            }
            Stmt::For(stmt) => {
                collect_task_group_async_lets_expr(&stmt.iterable, async_lets);
                collect_task_group_async_lets(&stmt.body, async_lets);
            }
            Stmt::Match(stmt) => {
                collect_task_group_async_lets_expr(&stmt.value, async_lets);
                for arm in &stmt.arms {
                    collect_task_group_async_lets(&arm.body, async_lets);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_task_group_async_lets_expr(&stmt.value, async_lets);
                collect_task_group_async_lets(&stmt.else_body, async_lets);
            }
            Stmt::Assign(stmt) => {
                collect_task_group_async_lets_expr(&stmt.target, async_lets);
                collect_task_group_async_lets_expr(&stmt.value, async_lets);
            }
            Stmt::Expr(expr) => collect_task_group_async_lets_expr(expr, async_lets),
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

fn collect_nested_task_group_async_lets(
    block: &Block,
    async_lets: &mut Vec<crate::diagnostic::Span>,
) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_all_async_let_spans_expr(value, async_lets);
                }
            }
            Stmt::With(stmt) => {
                collect_all_async_let_spans_expr(&stmt.resource, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::If(stmt) => {
                collect_all_async_let_spans_expr(&stmt.condition, async_lets);
                collect_all_async_let_spans(&stmt.then_body, async_lets);
                if let Some(else_body) = &stmt.else_body {
                    collect_all_async_let_spans(else_body, async_lets);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_all_async_let_spans_expr(condition, async_lets);
                }
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::For(stmt) => {
                collect_all_async_let_spans_expr(&stmt.iterable, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::Match(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                for arm in &stmt.arms {
                    collect_all_async_let_spans(&arm.body, async_lets);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                collect_all_async_let_spans(&stmt.else_body, async_lets);
            }
            Stmt::Assign(stmt) => {
                collect_all_async_let_spans_expr(&stmt.target, async_lets);
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
            }
            Stmt::Expr(expr) => collect_all_async_let_spans_expr(expr, async_lets),
            Stmt::Return(crate::syntax::ast::ReturnStmt {
                value: Some(expr), ..
            }) => {
                collect_all_async_let_spans_expr(expr, async_lets);
            }
            Stmt::Return(_)
            | Stmt::Select(_)
            | Stmt::TaskGroup(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

fn collect_all_async_let_spans(block: &Block, async_lets: &mut Vec<crate::diagnostic::Span>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.is_async {
                    async_lets.push(stmt.span.clone());
                }
                if let Some(value) = &stmt.value {
                    collect_all_async_let_spans_expr(value, async_lets);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_all_async_let_spans_expr(value, async_lets);
                }
            }
            Stmt::With(stmt) => {
                collect_all_async_let_spans_expr(&stmt.resource, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::If(stmt) => {
                collect_all_async_let_spans_expr(&stmt.condition, async_lets);
                collect_all_async_let_spans(&stmt.then_body, async_lets);
                if let Some(else_body) = &stmt.else_body {
                    collect_all_async_let_spans(else_body, async_lets);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_all_async_let_spans_expr(condition, async_lets);
                }
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::For(stmt) => {
                collect_all_async_let_spans_expr(&stmt.iterable, async_lets);
                collect_all_async_let_spans(&stmt.body, async_lets);
            }
            Stmt::Match(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                for arm in &stmt.arms {
                    collect_all_async_let_spans(&arm.body, async_lets);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
                collect_all_async_let_spans(&stmt.else_body, async_lets);
            }
            Stmt::Assign(stmt) => {
                collect_all_async_let_spans_expr(&stmt.target, async_lets);
                collect_all_async_let_spans_expr(&stmt.value, async_lets);
            }
            Stmt::Expr(expr) => collect_all_async_let_spans_expr(expr, async_lets),
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    collect_all_async_let_spans_expr(&arm.operation, async_lets);
                    collect_all_async_let_spans(&arm.body, async_lets);
                }
            }
            Stmt::TaskGroup(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

/// A directly-nested child of an expression: either another expression
/// (including the operand of `Await`/`Effect`/`Spawn`/etc.) or a block (closure
/// bodies and match-arm bodies).
enum AsyncExprChild<'a> {
    Expr(&'a Expr),
    Block(&'a Block),
}

/// Shared structural descent over an expression's children, in the canonical
/// order used by every async-let collector. `visit` is invoked once per direct
/// child and is responsible for its own recursion.
fn walk_async_expr_children<F>(expr: &Expr, mut visit: F)
where
    F: FnMut(AsyncExprChild<'_>),
{
    match expr {
        Expr::Binary { left, right, .. } => {
            visit(AsyncExprChild::Expr(left));
            visit(AsyncExprChild::Expr(right));
        }
        Expr::Field { base, .. } => visit(AsyncExprChild::Expr(base)),
        Expr::Index { base, index, .. } => {
            visit(AsyncExprChild::Expr(base));
            visit(AsyncExprChild::Expr(index));
        }
        Expr::Call { args, .. } => {
            for arg in args {
                visit(AsyncExprChild::Expr(&arg.value));
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => visit(AsyncExprChild::Expr(value)),
        Expr::Closure { body, .. } => visit(AsyncExprChild::Block(body)),
        Expr::Match { value, arms, .. } => {
            visit(AsyncExprChild::Expr(value));
            for arm in arms {
                visit(AsyncExprChild::Block(&arm.body));
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                visit(AsyncExprChild::Expr(&field.value));
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                visit(AsyncExprChild::Expr(&entry.key));
                visit(AsyncExprChild::Expr(&entry.value));
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                visit(AsyncExprChild::Expr(item));
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn collect_all_async_let_spans_expr(expr: &Expr, async_lets: &mut Vec<crate::diagnostic::Span>) {
    walk_async_expr_children(expr, |child| match child {
        AsyncExprChild::Expr(child) => collect_all_async_let_spans_expr(child, async_lets),
        AsyncExprChild::Block(block) => collect_all_async_let_spans(block, async_lets),
    });
}

fn collect_task_group_async_lets_expr(
    expr: &Expr,
    async_lets: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    walk_async_expr_children(expr, |child| match child {
        AsyncExprChild::Expr(child) => collect_task_group_async_lets_expr(child, async_lets),
        AsyncExprChild::Block(block) => collect_task_group_async_lets(block, async_lets),
    });
}

fn collect_task_group_awaited_handles(block: &Block, awaited: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_awaited_handles_expr(value, awaited);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_awaited_handles_expr(value, awaited);
                }
            }
            Stmt::With(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.resource, awaited);
                collect_task_group_awaited_handles(&stmt.body, awaited);
            }
            Stmt::If(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.condition, awaited);
                collect_task_group_awaited_handles(&stmt.then_body, awaited);
                if let Some(else_body) = &stmt.else_body {
                    collect_task_group_awaited_handles(else_body, awaited);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    collect_task_group_awaited_handles_expr(condition, awaited);
                }
                collect_task_group_awaited_handles(&stmt.body, awaited);
            }
            Stmt::For(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.iterable, awaited);
                collect_task_group_awaited_handles(&stmt.body, awaited);
            }
            Stmt::Match(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
                for arm in &stmt.arms {
                    collect_task_group_awaited_handles(&arm.body, awaited);
                }
            }
            Stmt::LetElse(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
                collect_task_group_awaited_handles(&stmt.else_body, awaited);
            }
            Stmt::Assign(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.target, awaited);
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
            }
            Stmt::Expr(expr) => collect_task_group_awaited_handles_expr(expr, awaited),
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

fn collect_direct_task_group_awaited_handles(block: &Block, awaited: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    collect_task_group_awaited_handles_expr(value, awaited);
                }
            }
            Stmt::Assign(stmt) => {
                collect_task_group_awaited_handles_expr(&stmt.target, awaited);
                collect_task_group_awaited_handles_expr(&stmt.value, awaited);
            }
            Stmt::Expr(expr) => collect_task_group_awaited_handles_expr(expr, awaited),
            _ => {}
        }
    }
}

fn direct_task_group_awaited_handles_in_stmt(
    statement: &Stmt,
) -> Vec<(String, crate::diagnostic::Span)> {
    let mut awaited = Vec::new();
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                direct_task_group_awaited_handles_in_expr(value, &mut awaited);
            }
        }
        Stmt::Assign(stmt) => {
            direct_task_group_awaited_handles_in_expr(&stmt.target, &mut awaited);
            direct_task_group_awaited_handles_in_expr(&stmt.value, &mut awaited);
        }
        Stmt::Expr(expr) => direct_task_group_awaited_handles_in_expr(expr, &mut awaited),
        _ => {}
    }
    awaited
}

fn direct_task_group_awaited_handles_in_expr(
    expr: &Expr,
    awaited: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    match expr {
        Expr::Await { value, span } => {
            if let Some(name) = await_handle_name(value) {
                awaited.push((name.to_string(), span.clone()));
            }
        }
        Expr::Try { value, .. } => direct_task_group_awaited_handles_in_expr(value, awaited),
        _ => {}
    }
}

fn find_nested_task_group_await_span<'a>(
    block: &'a Block,
    name: &str,
) -> Option<&'a crate::diagnostic::Span> {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::With(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.resource, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::If(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.condition, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.then_body, name) {
                    return Some(span);
                }
                if let Some(else_body) = &stmt.else_body
                    && let Some(span) = find_task_group_await_span(else_body, name)
                {
                    return Some(span);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition
                    && let Some(span) = find_nested_task_group_await_span_expr(condition, name)
                {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::For(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.iterable, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::Match(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                for arm in &stmt.arms {
                    if let Some(span) = find_task_group_await_span(&arm.body, name) {
                        return Some(span);
                    }
                }
            }
            Stmt::LetElse(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.else_body, name) {
                    return Some(span);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::Assign(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.target, name)
                    .or_else(|| find_nested_task_group_await_span_expr(&stmt.value, name))
                {
                    return Some(span);
                }
            }
            Stmt::Expr(expr) => {
                if let Some(span) = find_nested_task_group_await_span_expr(expr, name) {
                    return Some(span);
                }
            }
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
    None
}

fn find_task_group_await_span<'a>(
    block: &'a Block,
    name: &str,
) -> Option<&'a crate::diagnostic::Span> {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value
                    && let Some(span) = find_nested_task_group_await_span_expr(value, name)
                {
                    return Some(span);
                }
            }
            Stmt::With(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.resource, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::If(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.condition, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.then_body, name) {
                    return Some(span);
                }
                if let Some(else_body) = &stmt.else_body
                    && let Some(span) = find_task_group_await_span(else_body, name)
                {
                    return Some(span);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition
                    && let Some(span) = find_nested_task_group_await_span_expr(condition, name)
                {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::For(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.iterable, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.body, name) {
                    return Some(span);
                }
            }
            Stmt::Match(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                for arm in &stmt.arms {
                    if let Some(span) = find_task_group_await_span(&arm.body, name) {
                        return Some(span);
                    }
                }
            }
            Stmt::LetElse(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.value, name) {
                    return Some(span);
                }
                if let Some(span) = find_task_group_await_span(&stmt.else_body, name) {
                    return Some(span);
                }
            }
            Stmt::Assign(stmt) => {
                if let Some(span) = find_nested_task_group_await_span_expr(&stmt.target, name)
                    .or_else(|| find_nested_task_group_await_span_expr(&stmt.value, name))
                {
                    return Some(span);
                }
            }
            Stmt::Expr(expr) => {
                if let Some(span) = find_nested_task_group_await_span_expr(expr, name) {
                    return Some(span);
                }
            }
            Stmt::Select(_)
            | Stmt::TaskGroup(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
        }
    }
    None
}

fn find_nested_task_group_await_span_expr<'a>(
    expr: &'a Expr,
    name: &str,
) -> Option<&'a crate::diagnostic::Span> {
    match expr {
        Expr::Await { value, span } => {
            if await_handle_name(value).is_some_and(|handle| handle == name) {
                return Some(span);
            }
            find_nested_task_group_await_span_expr(value, name)
        }
        Expr::Binary { left, right, .. } => find_nested_task_group_await_span_expr(left, name)
            .or_else(|| find_nested_task_group_await_span_expr(right, name)),
        Expr::Field { base, .. } => find_nested_task_group_await_span_expr(base, name),
        Expr::Index { base, index, .. } => find_nested_task_group_await_span_expr(base, name)
            .or_else(|| find_nested_task_group_await_span_expr(index, name)),
        Expr::Call { args, .. } => args
            .iter()
            .find_map(|arg| find_nested_task_group_await_span_expr(&arg.value, name)),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => find_nested_task_group_await_span_expr(value, name),
        Expr::Closure { body, .. } => find_task_group_await_span(body, name),
        Expr::Match { value, arms, .. } => find_nested_task_group_await_span_expr(value, name)
            .or_else(|| {
                arms.iter()
                    .find_map(|arm| find_task_group_await_span(&arm.body, name))
            }),
        Expr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| find_nested_task_group_await_span_expr(&field.value, name)),
        Expr::MapLiteral { entries, .. } => entries.iter().find_map(|entry| {
            find_nested_task_group_await_span_expr(&entry.key, name)
                .or_else(|| find_nested_task_group_await_span_expr(&entry.value, name))
        }),
        Expr::ArrayLiteral { items, .. } => items
            .iter()
            .find_map(|item| find_nested_task_group_await_span_expr(item, name)),
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}

fn collect_task_group_awaited_handles_expr(expr: &Expr, awaited: &mut HashSet<String>) {
    // Record the awaited handle name before descending into the operand; the
    // structural descent itself (including into the `Await` operand) is shared.
    if let Expr::Await { value, .. } = expr
        && let Some(name) = await_handle_name(value)
    {
        awaited.insert(name.to_string());
    }
    walk_async_expr_children(expr, |child| match child {
        AsyncExprChild::Expr(child) => collect_task_group_awaited_handles_expr(child, awaited),
        AsyncExprChild::Block(block) => collect_task_group_awaited_handles(block, awaited),
    });
}

fn await_handle_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => await_handle_name(value),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeGuarantee {
    Noalloc,
    Pure,
    NoBlock,
    NoPanic,
}

impl RuntimeGuarantee {
    const ALL: [Self; 4] = [Self::Noalloc, Self::Pure, Self::NoBlock, Self::NoPanic];

    fn effect_name(self) -> &'static str {
        match self {
            Self::Noalloc => "noalloc",
            Self::Pure => "pure",
            Self::NoBlock => "no_block",
            Self::NoPanic => "no_panic",
        }
    }
}

impl Analyzer<'_> {
    pub(crate) fn resolve_type_alias<'b>(&'b self, type_name: &'b str) -> &'b str {
        let mut current = type_name;
        // Follow the alias chain with a depth limit to prevent infinite loops
        for _ in 0..16 {
            match self.type_aliases.get(current) {
                Some(resolved) => current = resolved.as_str(),
                None => break,
            }
        }
        current
    }

    /// Expand a type-alias reference, including generic aliases, to its target
    /// type. `IntList` -> `List<Int>`; `Pair<Int>` -> `Result<Int, String>` for
    /// `type Pair<T> = Result<T, String>`. Non-aliases pass through unchanged.
    pub(crate) fn expand_type_alias(&self, type_name: &str) -> String {
        use crate::text_util::{substitute_type_args, type_arg_names, type_root_name};
        let mut current = type_name.trim().to_string();
        for _ in 0..16 {
            let root = type_root_name(&current);
            let Some(target) = self.type_aliases.get(root) else {
                break;
            };
            let params = self
                .type_alias_params
                .get(root)
                .cloned()
                .unwrap_or_default();
            if params.is_empty() {
                current = target.clone();
            } else {
                // Generic alias: substitute the reference's arguments for the
                // alias's parameters. On arity mismatch, leave it for the normal
                // type checks to report.
                let Some(args) = type_arg_names(&current) else {
                    break;
                };
                if args.len() != params.len() {
                    break;
                }
                let subs: std::collections::HashMap<String, String> = params
                    .into_iter()
                    .zip(args.into_iter().map(str::to_string))
                    .collect();
                current = substitute_type_args(target, &subs);
            }
        }
        current
    }

    fn run(&mut self) {
        self.check_single_feature_declaration();
        self.check_unknown_file_features();
        self.check_duplicate_file_features();
        self.check_removed_profile_declarations();
        self.check_unsupported_syntax();
        self.check_derive_field_requirements();
        self.check_assignments();
        self.check_async_fn_lowerable();
        self.check_match_exhaustiveness();
        self.check_duplicate_declarations();
        self.check_lowered_name_conflicts();
        self.check_protocol_contracts();
        self.check_signature_explicitness();
        self.check_unknown_types();
        self.check_unknown_fields();
        self.check_unknown_bindings();
        self.check_fd_surface();
        self.check_generic_constraints();
        self.check_runtime_guarantee_bodies();
        self.check_try_operator_result_returns();
        self.check_resource_fields();
        self.check_weak_fields();
        self.check_resource_pool_type_arguments();
        self.check_resource_generic_arguments();
        checks::features::check(self);
        checks::calls::check(self);
        checks::body::check(self);
        checks::forbidden::check(self);
    }

    fn run_syntax_only(&mut self) {
        self.check_single_feature_declaration();
        self.check_unknown_file_features();
        self.check_duplicate_file_features();
        self.check_removed_profile_declarations();
        self.check_unsupported_syntax();
    }

    fn check_single_feature_declaration(&mut self) {
        for span in self.syntax_program.feature_spans.iter().skip(1) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_FEATURE_DECLARATION,
                    "RSScript files may declare at most one explicit feature header.",
                    span.clone(),
                    "duplicate features",
                )
                .with_cause("Only one top-level `features:` declaration is allowed.")
                .with_fix(
                    "remove_duplicate_features",
                    "Merge the feature list into one `features:` declaration.",
                    "manual",
                ),
            );
        }
    }

    fn check_unknown_file_features(&mut self) {
        for feature in &self.syntax_program.unknown_features {
            self.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_FILE_FEATURE,
                    format!("Unknown file feature `{}`.", feature.name),
                    feature.span.clone(),
                    "unknown feature",
                )
                .with_cause(
                    "File features must be review-relevant capabilities recognized by this compiler.",
                )
                .with_fix(
                    "remove_or_correct_feature",
                    "Remove the feature name or replace it with a supported feature such as `local`.",
                    "manual",
                ),
            );
        }
    }

    fn check_duplicate_file_features(&mut self) {
        for feature in &self.syntax_program.duplicate_features {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_FILE_FEATURE,
                    format!("Duplicate file feature `{}`.", feature.name),
                    feature.span.clone(),
                    "duplicate feature",
                )
                .with_cause(
                    "File features are capability declarations; repeating one makes the review boundary noisier without changing semantics.",
                )
                .with_fix(
                    "remove_duplicate_feature",
                    format!("Remove the repeated `{}` feature.", feature.name),
                    "machine-applicable",
                ),
            );
        }
    }

    fn check_removed_profile_declarations(&mut self) {
        for span in &self.syntax_program.profile_spans {
            self.diagnostics.push(
                Diagnostic::error(
                    code::REMOVED_PROFILE_DECLARATION,
                    "`profile:` declarations are not part of RSScript v0.6.",
                    span.clone(),
                    "removed profile declaration",
                )
                .with_cause("v0.6 uses `features:` for file-level advanced capabilities; omitted features means managed-only.")
                .with_fix(
                    "remove_profile",
                    "Remove `profile:` and add `features: local` only if the file uses local ownership features.",
                    "manual",
                ),
            );
        }
    }

    /// A user `async fn` lowers to a `Pending` chain. Control-flow statements
    /// with awaits lower as explicit statement boundaries; keep rejecting
    /// awaits embedded in ordinary expression positions where the lowering
    /// would otherwise hide an executor boundary inside an expression.
    fn check_async_fn_lowerable(&mut self) {
        use crate::syntax::ast::Item;
        let mut diagnostics = Vec::new();
        for item in &self.syntax_program.items {
            let Item::Function(function) = item else {
                continue;
            };
            if !function.is_async {
                continue;
            }
            if let Some(span) = async_block_nonlinear_await(&function.body) {
                diagnostics.push(async_not_lowerable_diagnostic(
                    span,
                    "this `await` is inside an expression that needs full async expression lowering",
                    "Move the await to a statement boundary, or put it inside an `if`/`loop`/`match` body where RSScript can create an explicit async boundary.",
                ));
            }
            // Independent of lowerability: a `Task.cancellation_token()` call in
            // A `Task.cancellation_token()` call in an async fn outside an
            // actual task_group still has no lexical cancellation owner, so it
            // would silently lower to a never-cancelled token. Reject it instead
            // of handing out fake structured cancellation.
            if let Some(span) = block_first_cancellation_token(&function.body) {
                diagnostics.push(cancellation_token_outside_task_group_diagnostic(span));
            }
        }
        self.diagnostics.extend(diagnostics);
    }

    /// Controlled ordinary assignment: `x = e` updates a `let mut` local. The
    /// left side must be a place whose root is a reassignable local, and `mut`
    /// must appear in the binding so mutation stays visible to the type system.
    fn check_assignments(&mut self) {
        use crate::syntax::ast::Item;
        let diagnostics = {
            let mut checker = AssignChecker::new(&self.hir);
            for item in &self.syntax_program.items {
                if let Item::Function(function) = item {
                    checker.check_function(function);
                }
            }
            checker.diagnostics
        };
        self.diagnostics.extend(diagnostics);
    }
}

fn resource_pool_namespace_arg(namespace: &str) -> Option<&str> {
    namespace
        .strip_prefix("ResourcePool<")
        .and_then(|rest| rest.strip_suffix('>'))
}

fn generic_namespace_args(namespace: &str) -> Option<(&str, Vec<&str>)> {
    let (root, rest) = namespace.split_once('<')?;
    let args = rest.strip_suffix('>')?;
    Some((root, split_top_level_type_args(args)))
}

/// The top-level parameter slices of a `Fn(...)` type string, e.g.
/// `owned Fn(read UOp, mut Ctx) -> Option<UOp>` → `["read UOp", "mut Ctx"]`.
/// Returns `None` when the string is not a `Fn` type.
fn fn_type_params(type_name: &str) -> Option<Vec<&str>> {
    let trimmed = type_name.trim();
    let after_prefix = trimmed
        .strip_prefix("fresh ")
        .unwrap_or(trimmed)
        .trim_start();
    let after_prefix = after_prefix
        .strip_prefix("owned ")
        .or_else(|| after_prefix.strip_prefix("noescape "))
        .unwrap_or(after_prefix)
        .trim_start();
    let inner = after_prefix.strip_prefix("Fn(")?;
    // Find the matching close paren of the parameter list.
    let mut depth = 1usize;
    let mut end = None;
    for (index, ch) in inner.char_indices() {
        match ch {
            '(' | '<' => depth += 1,
            ')' | '>' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let params = &inner[..end?];
    if params.trim().is_empty() {
        return Some(Vec::new());
    }
    Some(split_top_level_type_args(params))
}

/// The declared effect of each parameter in a `Fn(...)` type string, in order.
fn fn_type_param_effects(type_name: &str) -> Vec<Option<DataEffect>> {
    fn_type_params(type_name)
        .unwrap_or_default()
        .into_iter()
        .map(|param| match param.split_whitespace().next() {
            Some("read") => Some(DataEffect::Read),
            Some("mut") => Some(DataEffect::Mut),
            Some("take") => Some(DataEffect::Take),
            _ => None,
        })
        .collect()
}

/// The bare type name of the `index`-th parameter of a `Fn(...)` type string,
/// with any leading effect keyword stripped (`mut Ctx` → `Ctx`).
fn fn_type_param_type_name(type_name: &str, index: usize) -> Option<String> {
    let params = fn_type_params(type_name)?;
    let param = params.get(index)?.trim();
    let bare = param
        .strip_prefix("read ")
        .or_else(|| param.strip_prefix("mut "))
        .or_else(|| param.strip_prefix("take "))
        .unwrap_or(param);
    Some(bare.trim().to_string())
}

fn effect_name(effect: &EffectDecl) -> &str {
    match effect {
        EffectDecl::Name(name) | EffectDecl::Retains(name) => name,
    }
}

fn data_effect_name(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn effect_display(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn generic_bounds(params: &[GenericParam]) -> HashMap<String, Option<GenericBound>> {
    params
        .iter()
        .map(|param| (param.name.clone(), param.bound.clone()))
        .collect()
}

fn async_not_lowerable_diagnostic(
    span: crate::diagnostic::Span,
    label: &str,
    cause: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::ASYNC_FN_NOT_LOWERABLE,
        "async function is not lowerable in this version.",
        span,
        label,
    )
    .with_cause(cause)
    .with_fix(
        "restructure_async_fn",
        "Make every `await` a top-level statement, or move a `task_group` into a synchronous function.",
        "manual",
    )
}

fn cancellation_token_outside_task_group_diagnostic(span: crate::diagnostic::Span) -> Diagnostic {
    Diagnostic::error(
        code::CANCELLATION_TOKEN_OUTSIDE_TASK_GROUP,
        "`Task.cancellation_token()` is not allowed inside an `async fn`.",
        span,
        "this would observe a never-cancelled token, not the task_group's",
    )
    .with_cause(
        "An `async fn` has no lexically enclosing `task_group`, so this call cannot inherit the group's cancellation token and would silently never cancel.",
    )
    .with_fix(
        "pass_cancellation_token",
        "Call `Task.cancellation_token()` inside the `task_group` block and pass the token into this function as a `read CancellationToken` parameter.",
        "manual",
    )
}

/// First `Task.cancellation_token()` call span anywhere in a function body.
fn block_first_cancellation_token(block: &Block) -> Option<crate::diagnostic::Span> {
    block
        .statements
        .iter()
        .find_map(stmt_first_cancellation_token)
}

fn stmt_first_cancellation_token(statement: &Stmt) -> Option<crate::diagnostic::Span> {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().and_then(expr_first_cancellation_token),
        Stmt::Return(stmt) => stmt.value.as_ref().and_then(expr_first_cancellation_token),
        Stmt::Expr(expr) => expr_first_cancellation_token(expr),
        Stmt::With(stmt) => expr_first_cancellation_token(&stmt.resource)
            .or_else(|| block_first_cancellation_token(&stmt.body)),
        Stmt::If(stmt) => expr_first_cancellation_token(&stmt.condition)
            .or_else(|| block_first_cancellation_token(&stmt.then_body))
            .or_else(|| {
                stmt.else_body
                    .as_ref()
                    .and_then(block_first_cancellation_token)
            }),
        Stmt::Loop(stmt) => stmt
            .condition
            .as_ref()
            .and_then(expr_first_cancellation_token)
            .or_else(|| block_first_cancellation_token(&stmt.body)),
        Stmt::For(stmt) => expr_first_cancellation_token(&stmt.iterable)
            .or_else(|| block_first_cancellation_token(&stmt.body)),
        Stmt::Match(stmt) => expr_first_cancellation_token(&stmt.value).or_else(|| {
            stmt.arms
                .iter()
                .find_map(|arm| block_first_cancellation_token(&arm.body))
        }),
        Stmt::TaskGroup(_) => None,
        Stmt::LetElse(stmt) => expr_first_cancellation_token(&stmt.value)
            .or_else(|| block_first_cancellation_token(&stmt.else_body)),
        _ => None,
    }
}

fn expr_first_cancellation_token(expr: &Expr) -> Option<crate::diagnostic::Span> {
    match expr {
        Expr::Call { callee, args, span } => {
            if let Callee::Qualified { namespace, name } = callee
                && namespace == "Task"
                && name == "cancellation_token"
            {
                return Some(span.clone());
            }
            args.iter()
                .find_map(|arg| expr_first_cancellation_token(&arg.value))
        }
        Expr::Binary { left, right, .. } => {
            expr_first_cancellation_token(left).or_else(|| expr_first_cancellation_token(right))
        }
        Expr::Field { base, .. } => expr_first_cancellation_token(base),
        Expr::Index { base, index, .. } => {
            expr_first_cancellation_token(base).or_else(|| expr_first_cancellation_token(index))
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => expr_first_cancellation_token(value),
        Expr::Closure { body, .. } => block_first_cancellation_token(body),
        Expr::Match { value, arms, .. } => expr_first_cancellation_token(value).or_else(|| {
            arms.iter()
                .find_map(|arm| block_first_cancellation_token(&arm.body))
        }),
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| expr_first_cancellation_token(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| expr_first_cancellation_token(&entry.value))
            }),
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => None,
    }
}

/// AST analogue of [`crate::hir::assign_target_reads`]: the evaluated
/// sub-expressions of an assignment target (a field/index base, an index
/// expression), excluding the write root. So awaits/`?`/calls embedded in an
/// assignment *target* (e.g. `xs[await f()] = v`) are visited like the RHS.
fn assign_target_reads_ast(target: &Expr) -> Vec<&Expr> {
    match target {
        Expr::Ident(_, _) => Vec::new(),
        Expr::Field { base, .. } => vec![base.as_ref()],
        Expr::Index { base, index, .. } => vec![base.as_ref(), index.as_ref()],
        other => vec![other],
    }
}

/// The awaited inner expression of `await x` / `await x?`.
fn async_await_inner_ast(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Try { value, .. } => match value.as_ref() {
            Expr::Await { value, .. } => Some(value),
            _ => None,
        },
        Expr::Await { value, .. } => Some(value),
        _ => None,
    }
}

fn expr_first_await(expr: &Expr) -> Option<crate::diagnostic::Span> {
    match expr {
        Expr::Await { span, .. } => Some(span.clone()),
        Expr::Binary { left, right, .. } => {
            expr_first_await(left).or_else(|| expr_first_await(right))
        }
        Expr::Field { base, .. } => expr_first_await(base),
        Expr::Index { base, index, .. } => {
            expr_first_await(base).or_else(|| expr_first_await(index))
        }
        Expr::Call { args, .. } => args.iter().find_map(|arg| expr_first_await(&arg.value)),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => expr_first_await(value),
        Expr::Closure { body, .. } => block_first_await(body),
        Expr::Match { value, arms, .. } => expr_first_await(value)
            .or_else(|| {
                arms.iter()
                    .find_map(|arm| arm.guard.as_ref().and_then(expr_first_await))
            })
            .or_else(|| arms.iter().find_map(|arm| block_first_await(&arm.body))),
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| expr_first_await(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| expr_first_await(&entry.value))
            }),
        Expr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| expr_first_await(&field.value)),
        Expr::ArrayLiteral { items, .. } => items.iter().find_map(expr_first_await),
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => None,
    }
}

fn block_first_await(block: &Block) -> Option<crate::diagnostic::Span> {
    block.statements.iter().find_map(stmt_first_await_ast)
}

fn stmt_first_await_ast(statement: &Stmt) -> Option<crate::diagnostic::Span> {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().and_then(expr_first_await),
        Stmt::Return(stmt) => stmt.value.as_ref().and_then(expr_first_await),
        Stmt::Expr(expr) => expr_first_await(expr),
        Stmt::With(stmt) => {
            expr_first_await(&stmt.resource).or_else(|| block_first_await(&stmt.body))
        }
        Stmt::If(stmt) => expr_first_await(&stmt.condition)
            .or_else(|| block_first_await(&stmt.then_body))
            .or_else(|| stmt.else_body.as_ref().and_then(block_first_await)),
        Stmt::Loop(stmt) => stmt
            .condition
            .as_ref()
            .and_then(expr_first_await)
            .or_else(|| block_first_await(&stmt.body)),
        Stmt::For(stmt) => {
            if stmt.is_async {
                Some(stmt.span.clone())
            } else {
                expr_first_await(&stmt.iterable).or_else(|| block_first_await(&stmt.body))
            }
        }
        Stmt::Match(stmt) => expr_first_await(&stmt.value).or_else(|| {
            stmt.arms
                .iter()
                .find_map(|arm| block_first_await(&arm.body))
        }),
        Stmt::Select(stmt) => stmt.arms.iter().find_map(|arm| {
            expr_first_await(&arm.operation).or_else(|| block_first_await(&arm.body))
        }),
        Stmt::TaskGroup(stmt) => block_first_await(&stmt.body),
        Stmt::LetElse(stmt) => {
            expr_first_await(&stmt.value).or_else(|| block_first_await(&stmt.else_body))
        }
        Stmt::Assign(stmt) => assign_target_reads_ast(&stmt.target)
            .into_iter()
            .find_map(expr_first_await)
            .or_else(|| expr_first_await(&stmt.value)),
        _ => None,
    }
}

/// The span of the first `await` that is *not* a top-level statement of an async
/// body (i.e. nested in control flow or a non-await expression position).
fn async_block_nonlinear_await(block: &Block) -> Option<crate::diagnostic::Span> {
    for statement in &block.statements {
        let nested = match statement {
            Stmt::Let(stmt) => match stmt.value.as_ref() {
                Some(value) => match async_await_inner_ast(value) {
                    Some(inner) => expr_first_await(inner),
                    None => expr_first_await(value),
                },
                None => None,
            },
            Stmt::Expr(expr) => match async_await_inner_ast(expr) {
                Some(inner) => expr_first_await(inner),
                None => expr_first_await(expr),
            },
            // `return await op` is a top-level await; only nested awaits in the
            // returned expression are non-linear.
            Stmt::Return(stmt) => match stmt.value.as_ref() {
                Some(value) => match async_await_inner_ast(value) {
                    Some(inner) => expr_first_await(inner),
                    None => expr_first_await(value),
                },
                None => None,
            },
            Stmt::Select(_) | Stmt::For(_) | Stmt::TaskGroup(_) => None,
            Stmt::If(_) | Stmt::Loop(_) | Stmt::Match(_) | Stmt::With(_) => None,
            // An assignment's RHS follows the same rule as a `let` initializer (a
            // direct `await` is linear; nested awaits are not). The target is an
            // evaluated place, so any await inside a field/index base or index
            // expression is non-linear (e.g. `xs[await f()] = v`).
            Stmt::Assign(stmt) => assign_target_reads_ast(&stmt.target)
                .into_iter()
                .find_map(expr_first_await)
                .or_else(|| match async_await_inner_ast(&stmt.value) {
                    Some(inner) => expr_first_await(inner),
                    None => expr_first_await(&stmt.value),
                }),
            other => stmt_first_await_ast(other),
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

fn protocol_method_names(items: &[Item], protocol: &str) -> HashSet<String> {
    items
        .iter()
        .filter_map(|item| {
            let Item::Function(function) = item else {
                return None;
            };
            let (namespace, method) = split_qualified_name(&function.name);
            (namespace.as_deref() == Some(protocol)).then(|| method.to_string())
        })
        .collect()
}

fn function_body_belongs_to_protocol(
    function: &FunctionDecl,
    protocol_names: &HashSet<String>,
) -> bool {
    !function.body.statements.is_empty() && function_belongs_to_protocol(function, protocol_names)
}

fn function_belongs_to_protocol(function: &FunctionDecl, protocol_names: &HashSet<String>) -> bool {
    split_qualified_name(&function.name)
        .0
        .is_some_and(|namespace| protocol_names.contains(&namespace))
}

fn protocol_signature_mismatch(
    protocol: &FunctionSig,
    target: &FunctionSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.is_async != target.is_async {
        return Some("async/sync kind must match the protocol method exactly.".to_string());
    }
    if protocol.params.len() != target.params.len() {
        return Some(format!(
            "parameter count mismatch: protocol has {}, implementation has {}.",
            protocol.params.len(),
            target.params.len()
        ));
    }
    for (protocol_param, target_param) in protocol.params.iter().zip(&target.params) {
        if let Some(reason) = protocol_param_mismatch(protocol_param, target_param, concrete_type) {
            return Some(reason);
        }
    }
    let protocol_return = protocol
        .return_type
        .as_deref()
        .map(|return_type| substitute_protocol_self(return_type, concrete_type));
    if protocol_return.as_deref() != target.return_type.as_deref() {
        return Some(format!(
            "return type mismatch: protocol expects `{}`, implementation returns `{}`.",
            protocol_return.as_deref().unwrap_or("Unit"),
            target.return_type.as_deref().unwrap_or("Unit")
        ));
    }
    if protocol.returns_fresh != target.returns_fresh {
        return Some("fresh return mode must match the protocol method exactly.".to_string());
    }
    let protocol_effects = protocol.effects.iter().collect::<HashSet<_>>();
    let target_effects = target.effects.iter().collect::<HashSet<_>>();
    if protocol_effects != target_effects {
        return Some(
            "guarantee and boundary effects must match the protocol method exactly.".to_string(),
        );
    }
    if protocol.retained_params != target.retained_params {
        return Some("retains(...) effects must match the protocol method exactly.".to_string());
    }
    None
}

fn protocol_param_mismatch(
    protocol: &ParamSig,
    target: &ParamSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.name != target.name {
        return Some(format!(
            "parameter name mismatch: protocol expects `{}`, implementation has `{}`.",
            protocol.name, target.name
        ));
    }
    if protocol.effect != target.effect {
        return Some(format!(
            "parameter effect mismatch for `{}`: protocol expects `{}`, implementation has `{}`.",
            protocol.name,
            protocol
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none"),
            target
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none")
        ));
    }
    let expected_type = substitute_protocol_self(&protocol.type_name, concrete_type);
    if expected_type != target.type_name {
        return Some(format!(
            "parameter type mismatch for `{}`: protocol expects `{expected_type}`, implementation has `{}`.",
            protocol.name, target.type_name
        ));
    }
    None
}

fn substitute_protocol_self(type_name: &str, concrete_type: &str) -> String {
    if type_name == "Self" {
        return concrete_type.to_string();
    }
    // Use word-boundary-aware replacement to avoid substituting inside identifiers
    // like "MySelfThing". In RSScript, Self appears only as a standalone type or
    // inside generic brackets (e.g. "List<Self>", "Option<Self>").
    let mut result = String::new();
    let mut chars = type_name.char_indices().peekable();
    while let Some((i, _)) = chars.peek().copied() {
        if type_name[i..].starts_with("Self") {
            let before_ok = i == 0 || !type_name.as_bytes()[i - 1].is_ascii_alphanumeric();
            let after_pos = i + 4;
            let after_ok = after_pos >= type_name.len()
                || !type_name.as_bytes()[after_pos].is_ascii_alphanumeric();
            if before_ok && after_ok {
                result.push_str(concrete_type);
                for _ in 0..4 {
                    chars.next();
                }
                continue;
            }
        }
        result.push(chars.next().unwrap().1);
    }
    result
}

fn split_qualified_name(name: &str) -> (Option<String>, &str) {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        (Some(namespace.to_string()), name)
    } else {
        (None, name)
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

fn function_has_effect(function: &crate::syntax::ast::FunctionDecl, effect_name: &str) -> bool {
    function
        .effects
        .iter()
        .any(|effect| matches!(effect, EffectDecl::Name(name) if name == effect_name))
}

fn builtin_match_is_exhaustive(variants: &HashSet<String>) -> bool {
    (variants.contains("Some") && variants.contains("None"))
        || (variants.contains("Ok") && variants.contains("Err"))
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| crate::checks::shared::builtin_value_type_name(name)),
        HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Char { .. } => Some("Char"),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn constructor_pattern_is_irrefutable(pattern: &MatchPattern) -> bool {
    match pattern {
        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => true,
        // A variant pattern is irrefutable (over its own constructor) iff every
        // positional sub-pattern is itself irrefutable (a bare binder or `_`).
        // Payload-free (`bindings` empty) is trivially irrefutable.
        MatchPattern::Variant { bindings, .. } => bindings.iter().all(|binding| {
            matches!(
                binding,
                MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
            )
        }),
        MatchPattern::Struct { fields, .. } => fields.iter().all(|field| {
            field.pattern.is_none()
                || field.pattern.as_deref().is_some_and(|pattern| {
                    matches!(
                        pattern,
                        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                    )
                })
        }),
        // `[..]` / `[..rest]` matches any list; every other list pattern adds a
        // length or element constraint and so is refutable.
        MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } => prefix.is_empty() && suffix.is_empty() && rest.is_some(),
        MatchPattern::Literal { .. } => false,
    }
}

fn constrained_field_patterns(pattern: &MatchPattern) -> Vec<(String, &MatchPattern)> {
    match pattern {
        // Variant patterns are matched positionally against the constructor's
        // declared fields; see `pattern_matches_fields` in the exhaustiveness
        // module, which zips `bindings` with the witness fields by index.
        MatchPattern::Variant { .. } => Vec::new(),
        MatchPattern::Struct { fields, .. } => fields
            .iter()
            .filter_map(|field| {
                let pattern = field.pattern.as_deref()?;
                if matches!(
                    pattern,
                    MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                ) {
                    None
                } else {
                    Some((field.name.clone(), pattern))
                }
            })
            .collect(),
        MatchPattern::Binding { .. }
        | MatchPattern::Literal { .. }
        | MatchPattern::List { .. }
        | MatchPattern::Wildcard(_) => Vec::new(),
    }
}

fn callee_display(callee: &Callee) -> String {
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
            analyzer_expr_label(receiver)
        ),
    }
}

fn analyzer_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::CharLiteral(value, _) | Expr::MultilineString(value, _) => {
            format!("{value:?}")
        }
        Expr::Field { base, name, .. } => format!("{}.{}", analyzer_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", analyzer_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            analyzer_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

fn removed_runtime_effect_replacement(effect_name: &str) -> Option<&'static str> {
    match effect_name {
        "io" => Some(
            "Remove `io`; I/O is allowed by default unless a guarantee such as `pure` or `no_block` forbids it.",
        ),
        "allocates" => Some(
            "Remove `allocates`; allocation is allowed by default. Use `noalloc` only when the function guarantees no allocation.",
        ),
        "may_panic" => Some(
            "Remove `may_panic`; panic is allowed by default. Use `no_panic` only when the function guarantees no panic.",
        ),
        "may_fail" => Some(
            "Remove `may_fail`; represent failure in the return type, for example `Result<T, E>`.",
        ),
        "async" => Some(
            "Remove `async` from `effects(...)`; write `async fn` when the function itself is async.",
        ),
        "suspends" => Some(
            "`suspends` is compiler-normalized review metadata for `async fn`; remove it from `effects(...)` and write `async fn` on the function.",
        ),
        _ => None,
    }
}

fn item_span(item: &Item) -> &crate::diagnostic::Span {
    match item {
        Item::Module(decl) => &decl.span,
        Item::Use(decl) => &decl.span,
        Item::Type(decl) => &decl.span,
        Item::SumType(sum) => &sum.span,
        Item::TypeAlias(alias) => &alias.span,
        Item::Const(decl) => &decl.span,
        Item::Function(function) => &function.span,
    }
}

fn duplicate_symbol_label(kind: DuplicateSymbolKind) -> &'static str {
    match kind {
        DuplicateSymbolKind::Function => "function",
        DuplicateSymbolKind::Type => "type",
        DuplicateSymbolKind::Constructor => "callable",
        DuplicateSymbolKind::Field => "field",
    }
}

fn is_copy_type(ty: &TypeRef) -> bool {
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

fn known_type_ref(ty: &TypeRef, generic_params: &HashSet<&str>, hir: &Hir) -> bool {
    if ty.name.is_empty() {
        return true;
    }
    if ty.is_noescape || ty.is_owned {
        return ty.name == "Fn";
    }
    generic_params.contains(ty.name.as_str())
        || is_builtin_type_name(&ty.name)
        || hir.type_info(&ty.name).is_some()
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "Unit"
            | "Bool"
            | "Byte"
            | "Char"
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
            | "Float"
            | "Float32"
            | "Float64"
            | "String"
            | "StringView"
            | "Url"
            | "Fd"
            | "Bytes"
            | "BytesView"
            | "Buffer"
            | "BufferView"
            | "Path"
            | "Result"
            | "Option"
            | "List"
            | "Map"
            | "Set"
            | "Capability"
            | "Fn"
            | "Closure"
            | "Cache"
            | "FileError"
            | "IOError"
            | "HttpError"
            | "ConfigError"
            | "DbError"
            | "ImageError"
            | "JsonError"
            | "CsvError"
            | "NetworkError"
    )
}

fn builtin_value_ident(name: &str) -> bool {
    matches!(name, "true" | "false" | "Unit" | "None" | "null")
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

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = match ty.fn_param_effects.get(index).copied().flatten() {
                    Some(effect) => format!("{} ", effect.as_str()),
                    None => String::new(),
                };
                format!("{prefix}{}", type_ref_name(param))
            })
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
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
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

/// The originating source file of a top-level item (from its span).
fn item_span_file(item: &Item) -> String {
    match item {
        Item::Function(decl) => decl.span.file.clone(),
        Item::Const(decl) => decl.span.file.clone(),
        Item::Type(decl) => decl.span.file.clone(),
        Item::SumType(decl) => decl.span.file.clone(),
        Item::TypeAlias(decl) => decl.span.file.clone(),
        Item::Module(decl) => decl.span.file.clone(),
        Item::Use(decl) => decl.span.file.clone(),
    }
}

/// Whether `name` is a plain Rust identifier (used verbatim as a pinned backend
/// symbol): non-empty, starts with a letter or `_`, and otherwise alphanumeric or
/// `_`. Raw identifiers and keywords are intentionally rejected.
fn is_valid_rust_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !crate::rust_lower::is_rust_keyword(name)
}

fn type_ref_is_noescape(ty: &TypeRef) -> bool {
    ty.is_noescape || ty.args.iter().any(type_ref_is_noescape)
}

fn type_ref_is_owned(ty: &TypeRef) -> bool {
    ty.is_owned || ty.args.iter().any(type_ref_is_owned)
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

fn type_ref_is_closure_effect_exempt(ty: &TypeRef) -> bool {
    ty.args.is_empty() && !ty.is_noescape && ty.name == "Closure"
}

fn type_ref_has_surface_reference(ty: &TypeRef, tokens: &[Token]) -> bool {
    tokens.iter().any(|token| {
        token.symbol("&")
            && token.span.file == ty.span.file
            && token.span.line == ty.span.line
            && token.span.column + token.span.length <= ty.span.column
    })
}
