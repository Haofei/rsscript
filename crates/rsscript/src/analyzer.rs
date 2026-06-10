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
    let syntax_program = parse_source(file, source);
    let hir = Hir::from_syntax_with_standard_package_interfaces(&syntax_program);
    analyze_program(tokens, syntax_program, hir, builtin_interface_programs())
}

pub fn analyze_syntax_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let syntax_program = parse_source(file, source);
    let hir = Hir::from_syntax(&syntax_program);
    analyze_syntax_program(tokens, syntax_program, hir)
}

pub fn analyze_source_without_core(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let syntax_program = parse_source(file, source);
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
    let syntax_program = parse_source(file, source);
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
    let syntax_program = parse_source(file, source);
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
    let syntax_program = merge_programs(
        sources
            .iter()
            .map(|(file, source)| parse_source(file, source)),
    );
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
    let syntax_program = merge_programs(
        sources
            .iter()
            .map(|(file, source)| parse_source(file, source)),
    );
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
    let mut analyzer = Analyzer {
        tokens: &tokens,
        syntax_program,
        interface_programs,
        hir,
        diagnostics: Vec::new(),
        type_aliases,
        in_task_group: false,
        async_let_names: Vec::new(),
    };
    analyzer.run();
    analyzer.diagnostics
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
        in_task_group: false,
        async_let_names: Vec::new(),
    };
    analyzer.run_syntax_only();
    analyzer.diagnostics
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

fn collect_all_async_let_spans_expr(expr: &Expr, async_lets: &mut Vec<crate::diagnostic::Span>) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_all_async_let_spans_expr(left, async_lets);
            collect_all_async_let_spans_expr(right, async_lets);
        }
        Expr::Field { base, .. } => collect_all_async_let_spans_expr(base, async_lets),
        Expr::Index { base, index, .. } => {
            collect_all_async_let_spans_expr(base, async_lets);
            collect_all_async_let_spans_expr(index, async_lets);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_all_async_let_spans_expr(&arg.value, async_lets);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => collect_all_async_let_spans_expr(value, async_lets),
        Expr::Closure { body, .. } => collect_all_async_let_spans(body, async_lets),
        Expr::Match { value, arms, .. } => {
            collect_all_async_let_spans_expr(value, async_lets);
            for arm in arms {
                collect_all_async_let_spans(&arm.body, async_lets);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_all_async_let_spans_expr(&field.value, async_lets);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_all_async_let_spans_expr(&entry.key, async_lets);
                collect_all_async_let_spans_expr(&entry.value, async_lets);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_all_async_let_spans_expr(item, async_lets);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn collect_task_group_async_lets_expr(
    expr: &Expr,
    async_lets: &mut Vec<(String, crate::diagnostic::Span)>,
) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_task_group_async_lets_expr(left, async_lets);
            collect_task_group_async_lets_expr(right, async_lets);
        }
        Expr::Field { base, .. } => collect_task_group_async_lets_expr(base, async_lets),
        Expr::Index { base, index, .. } => {
            collect_task_group_async_lets_expr(base, async_lets);
            collect_task_group_async_lets_expr(index, async_lets);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_task_group_async_lets_expr(&arg.value, async_lets);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => collect_task_group_async_lets_expr(value, async_lets),
        Expr::Closure { body, .. } => collect_task_group_async_lets(body, async_lets),
        Expr::Match { value, arms, .. } => {
            collect_task_group_async_lets_expr(value, async_lets);
            for arm in arms {
                collect_task_group_async_lets(&arm.body, async_lets);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_task_group_async_lets_expr(&field.value, async_lets);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_task_group_async_lets_expr(&entry.key, async_lets);
                collect_task_group_async_lets_expr(&entry.value, async_lets);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_task_group_async_lets_expr(item, async_lets);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
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
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}

fn collect_task_group_awaited_handles_expr(expr: &Expr, awaited: &mut HashSet<String>) {
    match expr {
        Expr::Await { value, .. } => {
            if let Some(name) = await_handle_name(value) {
                awaited.insert(name.to_string());
            }
            collect_task_group_awaited_handles_expr(value, awaited);
        }
        Expr::Binary { left, right, .. } => {
            collect_task_group_awaited_handles_expr(left, awaited);
            collect_task_group_awaited_handles_expr(right, awaited);
        }
        Expr::Field { base, .. } => collect_task_group_awaited_handles_expr(base, awaited),
        Expr::Index { base, index, .. } => {
            collect_task_group_awaited_handles_expr(base, awaited);
            collect_task_group_awaited_handles_expr(index, awaited);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_task_group_awaited_handles_expr(&arg.value, awaited);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => collect_task_group_awaited_handles_expr(value, awaited),
        Expr::Closure { body, .. } => collect_task_group_awaited_handles(body, awaited),
        Expr::Match { value, arms, .. } => {
            collect_task_group_awaited_handles_expr(value, awaited);
            for arm in arms {
                collect_task_group_awaited_handles(&arm.body, awaited);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_task_group_awaited_handles_expr(&field.value, awaited);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_task_group_awaited_handles_expr(&entry.key, awaited);
                collect_task_group_awaited_handles_expr(&entry.value, awaited);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_task_group_awaited_handles_expr(item, awaited);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
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

    fn check_unsupported_syntax(&mut self) {
        for span in self.syntax_program.unknown_top_level_spans.clone() {
            self.unsupported_syntax(
                span,
                "unsupported top-level item",
                "This top-level construct is outside the current RSScript parser surface.",
            );
        }
        for span in self.syntax_program.malformed_declaration_spans.clone() {
            self.unsupported_syntax(
                span,
                "malformed declaration",
                "This declaration starts like RSScript syntax but does not match the supported declaration grammar.",
            );
        }
        self.check_module_use_layout();
        let items = self.syntax_program.items.clone();
        for item in &items {
            self.check_unsupported_syntax_item(item);
        }
    }

    fn check_module_use_layout(&mut self) {
        let mut seen_module = false;
        let mut seen_use = false;
        let mut seen_non_organization_item = false;
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Module(module) => {
                    if seen_module {
                        self.unsupported_syntax(
                            module.span.clone(),
                            "duplicate module declaration",
                            "A source or interface file may declare at most one `module` identity.",
                        );
                    }
                    if seen_non_organization_item {
                        self.unsupported_syntax(
                            module.span.clone(),
                            "misplaced module declaration",
                            "`module` is source-organization metadata and must appear before declarations.",
                        );
                    }
                    if seen_use {
                        self.unsupported_syntax(
                            module.span.clone(),
                            "misplaced module declaration",
                            "`module` must be the first organization declaration when present; `use` declarations follow it.",
                        );
                    }
                    seen_module = true;
                }
                Item::Use(use_decl) => {
                    if seen_non_organization_item {
                        self.unsupported_syntax(
                            use_decl.span.clone(),
                            "misplaced use declaration",
                            "`use` is source-organization metadata and must appear before declarations.",
                        );
                    }
                    seen_use = true;
                }
                Item::Type(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_)
                | Item::Function(_) => {
                    seen_non_organization_item = true;
                }
            }
        }
    }

    fn check_unsupported_syntax_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => {
                for span in &function.malformed_generic_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed generic parameter declaration",
                        "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                    );
                }
                if function.is_native && !function.body.statements.is_empty() {
                    self.unsupported_syntax(
                        function.span.clone(),
                        "unsupported native function body",
                        "`native fn` declares an external/native boundary in v0.6. Provide a bodyless declaration and bind the implementation through the native wrapper path.",
                    );
                }
                for span in &function.malformed_effect_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed effect declaration",
                        "Effects must use a bare effect name or `retains(parameter)`.",
                    );
                }
                for span in &function.malformed_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed parameter declaration",
                        "Function parameters must use `name: Type`, `name: read Type`, `name: mut Type`, or `name: take Type`.",
                    );
                }
                for param in &function.params {
                    self.check_unsupported_syntax_type_ref(&param.ty, true, true);
                }
                if let Some(return_ty) = &function.return_ty {
                    self.check_unsupported_syntax_type_ref(return_ty, false, false);
                }
                self.check_unsupported_syntax_block(&function.body);
            }
            Item::Type(type_decl) => {
                self.check_supported_derives(&type_decl.derives, &type_decl.span);
                if type_decl.kind == TypeKind::Resource {
                    self.check_resource_derives(&type_decl.derives, &type_decl.span);
                }
                for span in &type_decl.malformed_generic_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed generic parameter declaration",
                        "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                    );
                }
                for span in &type_decl.malformed_field_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed field declaration",
                        "Type fields must use `name: Type`, `name: handle Type`, or `name: weak Type`.",
                    );
                }
                if type_decl.is_opaque && !type_decl.fields.is_empty() {
                    self.unsupported_syntax(
                        type_decl.span.clone(),
                        "unsupported opaque type body",
                        "Opaque interface types hide their representation. Declare `opaque struct Name`, `opaque class Name`, or `opaque resource Name` without fields.",
                    );
                }
                if type_decl.is_opaque && type_decl.drop_body.is_some() {
                    self.unsupported_syntax(
                        type_decl.span.clone(),
                        "unsupported opaque type body",
                        "Opaque resource contracts hide their implementation details, including drop bodies. Resource cleanup belongs to the implementation, not the `.rssi` contract.",
                    );
                }
                for field in &type_decl.fields {
                    self.check_unsupported_syntax_type_ref(&field.ty, false, false);
                }
                if type_decl.kind != TypeKind::Resource
                    && let Some(drop_body) = &type_decl.drop_body
                {
                    self.unsupported_syntax(
                        drop_body.span.clone(),
                        "unsupported managed drop",
                        "Managed class and struct values do not have user-observable destructors in v0.6. Use `resource` with `with` or `ResourcePool` for deterministic cleanup.",
                    );
                }
            }
            Item::SumType(sum) => {
                self.check_supported_derives(&sum.derives, &sum.span);
            }
            Item::Module(_) | Item::Use(_) | Item::TypeAlias(_) | Item::Const(_) => {}
        }
    }

    fn check_supported_derives(&mut self, derives: &[String], span: &crate::diagnostic::Span) {
        for derive in derives {
            if supported_compiler_derive(derive) {
                continue;
            }
            self.unsupported_syntax(
                span.clone(),
                "unsupported derive",
                "This name is not a compiler-owned RSScript derive. Supported derives are Debug, Clone, Eq, Ord, Hash, JsonEncode, JsonDecode, Schema, and ReviewSchema.",
            );
        }
    }

    /// Resources are move-only RAII values that default to `Debug` only. Value
    /// derives (`Clone`, `Eq`, `Ord`, `Hash`, `JsonEncode`, `JsonDecode`) would
    /// expand into generated Rust that copies or compares the resource — which
    /// contradicts the move-only model and can leak a backend trait-bound error
    /// — so only the implicit `Debug` and the review-only markers are allowed.
    fn check_resource_derives(&mut self, derives: &[String], span: &crate::diagnostic::Span) {
        for derive in derives {
            // Unknown names are already reported by `check_supported_derives`.
            if !supported_compiler_derive(derive)
                || matches!(derive.as_str(), "Debug" | "Schema" | "ReviewSchema")
            {
                continue;
            }
            self.diagnostics.push(
                Diagnostic::error(
                    code::RESOURCE_DERIVE_UNSUPPORTED,
                    format!("`{derive}` derive is not allowed on a resource."),
                    span.clone(),
                    "resources support only `Debug`, `Schema`, and `ReviewSchema`",
                )
                .with_cause(
                    "Resources are move-only RAII values. Value derives such as `Clone`, `Eq`, `Ord`, `Hash`, `JsonEncode`, and `JsonDecode` would copy or compare the resource, which contradicts its move-only model.",
                )
                .with_fix(
                    "remove_resource_derive",
                    format!("Remove `{derive}` from the resource, or model the data as a `struct`."),
                    "manual",
                ),
            );
        }
    }

    /// A compiler-owned derive expands to generated Rust, so every field must
    /// support it. This reports an RSScript diagnostic before lowering instead
    /// of letting a `Float: Eq` style trait-bound error leak from rustc.
    fn check_derive_field_requirements(&mut self) {
        use crate::syntax::ast::Item;
        let local_types = self.collect_local_value_types();
        let mut violations: Vec<(crate::diagnostic::Span, String, String, &'static str)> =
            Vec::new();

        for item in &self.syntax_program.items {
            let (derives, type_params, field_sets): (
                &[String],
                &[GenericParam],
                Vec<&[FieldDecl]>,
            ) = match item {
                // Resources default to `Debug` only and hold special internal
                // fields, so they are not part of structural-derive checking.
                Item::Type(type_decl) if type_decl.kind != TypeKind::Resource => (
                    &type_decl.derives,
                    &type_decl.type_params,
                    vec![type_decl.fields.as_slice()],
                ),
                Item::SumType(sum) => (
                    &sum.derives,
                    &sum.type_params,
                    sum.variants
                        .iter()
                        .map(|variant| variant.fields.as_slice())
                        .collect(),
                ),
                _ => continue,
            };

            let required = required_field_derives(derives);
            if required.is_empty() {
                continue;
            }
            let generic_params: HashSet<&str> = type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect();

            for fields in &field_sets {
                for field in fields.iter() {
                    // `handle`/`weak` fields lower to `Managed<T>`/`WeakManaged<T>`,
                    // which implement only `Clone`/`Debug` — never `Eq`, `Ord`,
                    // `Hash`, or serde — so any required derive is unsupported.
                    let managed = field.is_handle || field.is_weak;
                    for &derive in &required {
                        let supported = if managed {
                            DeriveSupport::No
                        } else {
                            field_supports_derive(&field.ty, derive, &local_types, &generic_params)
                        };
                        if supported == DeriveSupport::No {
                            let type_name = if field.is_handle {
                                format!("handle {}", type_ref_name(&field.ty))
                            } else if field.is_weak {
                                format!("weak {}", type_ref_name(&field.ty))
                            } else {
                                type_ref_name(&field.ty)
                            };
                            violations.push((
                                field.span.clone(),
                                field.name.clone(),
                                type_name,
                                derive,
                            ));
                        }
                    }
                }
            }
        }

        for (span, field_name, type_name, derive) in violations {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DERIVE_FIELD_UNSUPPORTED,
                    format!("`{derive}` derive is not supported by field `{field_name}`."),
                    span,
                    format!("`{type_name}` does not support the `{derive}` derive"),
                )
                .with_cause(derive_requirement_cause(derive))
                .with_fix(
                    "remove_or_change_derive",
                    format!(
                        "Remove `{derive}` from the derive list, or change `{field_name}` to a type that supports it."
                    ),
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

    /// Map locally declared type names to whether they are value types (struct
    /// or sum) and which derives they carry. Only locally declared types get a
    /// definite verdict; interface/runtime types have hand-written impls and are
    /// left to the backend.
    fn collect_local_value_types(&self) -> HashMap<String, LocalTypeDerives> {
        use crate::syntax::ast::Item;
        let mut map = HashMap::new();
        for item in &self.syntax_program.items {
            match item {
                Item::Type(type_decl) => {
                    map.insert(
                        type_decl.name.clone(),
                        LocalTypeDerives {
                            is_value: type_decl.kind == TypeKind::Struct,
                            derives: type_decl.derives.iter().cloned().collect(),
                        },
                    );
                }
                Item::SumType(sum) => {
                    map.insert(
                        sum.name.clone(),
                        LocalTypeDerives {
                            is_value: true,
                            derives: sum.derives.iter().cloned().collect(),
                        },
                    );
                }
                _ => {}
            }
        }
        map
    }

    fn check_unsupported_syntax_type_ref(
        &mut self,
        ty: &TypeRef,
        allow_noescape_param: bool,
        allow_owned_param: bool,
    ) {
        if ty.is_noescape && (!allow_noescape_param || ty.name != "Fn") {
            self.unsupported_syntax(
                ty.span.clone(),
                "unsupported noescape position",
                "`noescape Fn(...)` is only supported as a direct function parameter type.",
            );
        }
        if ty.is_owned && (!allow_owned_param || ty.name != "Fn") {
            self.unsupported_syntax(
                ty.span.clone(),
                "unsupported owned position",
                "`owned Fn(...)` is only supported as a direct function parameter type.",
            );
        }
        for span in &ty.malformed_arg_spans {
            self.unsupported_syntax(
                span.clone(),
                "malformed type argument",
                "Type arguments must be valid type references; empty or unsupported type argument slots are not allowed.",
            );
        }
        for arg in &ty.args {
            self.check_unsupported_syntax_type_ref(arg, false, false);
        }
    }

    fn check_unsupported_syntax_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.check_unsupported_syntax_stmt(statement);
        }
    }

    fn check_unsupported_syntax_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.malformed {
                    self.unsupported_syntax(
                        stmt.span.clone(),
                        "malformed statement",
                        "`let` and `local` bindings need a binding name, and an `=` must be followed by an expression.",
                    );
                }
               if stmt.is_async && !self.in_task_group {
                   self.unsupported_syntax(
                       stmt.span.clone(),
                       "`async let` outside task_group",
                       "`async let` can only be used inside a `task_group { ... }` block.",
                   );
               }
               if let Some(value) = &stmt.value {
                   self.check_unsupported_syntax_expr(value);
               }
           }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_unsupported_syntax_expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.resource);
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedWith(span) => self.unsupported_syntax(
                span.clone(),
                "malformed with statement",
                "`with` statements must use `with resource as name { ... }`.",
            ),
            Stmt::If(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.condition);
                self.check_unsupported_syntax_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_unsupported_syntax_block(else_body);
                }
            }
            Stmt::MalformedIf(span) => self.unsupported_syntax(
                span.clone(),
                "malformed if statement",
                "`if` statements must use `if condition { ... }` with optional `else { ... }` or `else if ...`.",
            ),
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_unsupported_syntax_expr(condition);
                }
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedLoop(span) => self.unsupported_syntax(
                span.clone(),
                "malformed loop statement",
                "`loop` statements must use `loop { ... }`; `while` statements must use `while condition { ... }`.",
            ),
            Stmt::For(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.iterable);
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_task_group_async_let_shape(&stmt.body);
                self.check_task_group_async_lets_consumed(&stmt.body);
                let was_in_task_group = self.in_task_group;
                self.in_task_group = true;
                self.check_unsupported_syntax_block(&stmt.body);
                self.in_task_group = was_in_task_group;
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    if async_await_inner_ast(&arm.operation).is_none() {
                        self.unsupported_syntax(
                            arm.span.clone(),
                            "malformed select arm",
                            "Select arms must use `name = await operation => { ... }`.",
                        );
                    }
                    self.check_unsupported_syntax_expr(&arm.operation);
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Stmt::MalformedFor(span) => self.unsupported_syntax(
                span.clone(),
                "malformed for statement",
                "`for` statements must use `for name in iterable { ... }`.",
            ),
            Stmt::Match(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.value);
                for span in &stmt.malformed_arm_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed match arm",
                        "Match arms must use `pattern => statement` or `pattern => { ... }`.",
                    );
                }
                for arm in &stmt.arms {
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.value);
                self.check_unsupported_syntax_block(&stmt.else_body);
            }
            Stmt::MalformedMatch(span) => self.unsupported_syntax(
                span.clone(),
                "malformed match statement",
                "`match` statements must use `match value { pattern => ... }`.",
            ),
            Stmt::Assign(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.target);
                self.check_unsupported_syntax_expr(&stmt.value);
            }
            Stmt::Expr(expr) => self.check_unsupported_syntax_expr(expr),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(span) => self.unsupported_syntax(
                span.clone(),
                "unsupported statement",
                "This statement is outside the current RSScript parser surface.",
            ),
        }
    }

    fn check_unsupported_syntax_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.check_unsupported_syntax_expr(left);
                self.check_unsupported_syntax_expr(right);
            }
            Expr::Field { base, .. } => self.check_unsupported_syntax_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_unsupported_syntax_expr(base);
                self.check_unsupported_syntax_expr(index);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    if arg.malformed {
                        self.unsupported_syntax(
                            arg.span.clone(),
                            "malformed call argument",
                            "Call arguments cannot contain empty argument slots.",
                        );
                    } else {
                        self.check_unsupported_syntax_expr(&arg.value);
                    }
                }
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Spawn { value, span } => {
                self.unsupported_syntax(
                    span.clone(),
                    "unsupported spawn expression",
                    "`spawn` is not a v0.6 source-level task feature. Use `task_group { async let ... }` for structured isolate-local async work.",
                );
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Await { value, .. } => {
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Closure { body, .. } => self.check_unsupported_syntax_block(body),
            Expr::Match { value, arms, .. } => {
                self.check_unsupported_syntax_expr(value);
                for arm in arms {
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Expr::ObjectLiteral { fields, .. } => {
                for field in fields {
                    self.check_unsupported_syntax_expr(&field.value);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_unsupported_syntax_expr(&entry.key);
                    self.check_unsupported_syntax_expr(&entry.value);
                }
            }
            Expr::ArrayLiteral { items, .. } => {
                for item in items {
                    self.check_unsupported_syntax_expr(item);
                }
            }
            Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::MultilineString(_, _) => {}
            Expr::Unknown(span) => self.unsupported_syntax(
                span.clone(),
                "unsupported expression",
                "This expression is outside the current RSScript parser surface.",
            ),
        }
    }

    fn check_task_group_async_lets_consumed(&mut self, block: &Block) {
        let mut async_lets = Vec::new();
        let mut awaited = HashSet::new();
        collect_task_group_async_lets(block, &mut async_lets);
        collect_direct_task_group_awaited_handles(block, &mut awaited);
        for (name, span) in async_lets {
            if name == "_" {
                continue;
            }
            if !awaited.contains(&name) {
                self.unsupported_syntax(
                    span,
                    "unawaited async let",
                    "`async let` handles are lexical task_group handles and must be consumed by `await` inside the same `task_group { ... }` block.",
                );
            }
        }
    }

    fn check_task_group_async_let_shape(&mut self, block: &Block) {
        let mut top_level_async_lets = HashSet::new();
        for statement in &block.statements {
            if let Stmt::Let(stmt) = statement
                && stmt.is_async
                && stmt.name != "_"
            {
                top_level_async_lets.insert(stmt.name.clone());
            }
        }

        let mut nested_async_lets = Vec::new();
        collect_nested_task_group_async_lets(block, &mut nested_async_lets);
        for span in nested_async_lets {
            self.unsupported_syntax(
                span,
                "nested async let",
                "`async let` is currently supported only as a direct child of `task_group { ... }` so checking and lowering share one structured-concurrency model.",
            );
        }

        let mut all_awaited = HashSet::new();
        let mut direct_awaited = HashSet::new();
        collect_task_group_awaited_handles(block, &mut all_awaited);
        collect_direct_task_group_awaited_handles(block, &mut direct_awaited);
        for name in all_awaited {
            if top_level_async_lets.contains(&name) && !direct_awaited.contains(&name) {
                let span = find_nested_task_group_await_span(block, &name)
                    .cloned()
                    .unwrap_or_else(|| block.span.clone());
                self.unsupported_syntax(
                    span,
                    "nested async let await",
                    "`await` of a task_group async-let handle must be a direct task_group statement in the v0.6 executable MVP.",
                );
            }
        }

        let mut declared = HashSet::new();
        let mut awaited = HashSet::new();
        for statement in &block.statements {
            if let Stmt::Let(stmt) = statement
                && stmt.is_async
            {
                if stmt.name != "_" {
                    declared.insert(stmt.name.clone());
                }
                continue;
            }

            for (name, span) in direct_task_group_awaited_handles_in_stmt(statement) {
                if !top_level_async_lets.contains(&name) {
                    continue;
                }
                if !declared.contains(&name) {
                    self.unsupported_syntax(
                        span,
                        "async let await before declaration",
                        "`await` of a task_group async-let handle must appear after the matching `async let` declaration.",
                    );
                } else if !awaited.insert(name) {
                    self.unsupported_syntax(
                        span,
                        "async let awaited more than once",
                        "`async let` handles are bounded task_group handles and can be consumed by `await` only once.",
                    );
                }
            }
        }
    }

    fn unsupported_syntax(&mut self, span: crate::diagnostic::Span, label: &str, cause: &str) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNSUPPORTED_SYNTAX,
                "unsupported RSScript syntax.",
                span,
                label,
            )
            .with_cause(cause)
            .with_fix(
                "rewrite_supported_syntax",
                "Rewrite this construct using the currently supported RSScript syntax.",
                "manual",
            ),
        );
    }

    fn check_match_exhaustiveness(&mut self) {
        let function_names = self
            .syntax_program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for function_name in function_names {
            let Some(body) = self
                .hir
                .function_body(&function_name)
                .and_then(|body| body.block.clone())
            else {
                continue;
            };
            self.check_match_exhaustiveness_block(&body);
        }
    }

    fn check_match_exhaustiveness_block(&mut self, block: &HirBlock) {
        for statement in &block.statements {
            self.check_match_exhaustiveness_stmt(statement);
        }
    }

    fn check_match_exhaustiveness_stmt(&mut self, statement: &HirStmt) {
        match statement {
            HirStmt::Let { value, .. } => {
                if let Some(value) = value {
                    self.check_match_exhaustiveness_expr(value);
                }
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.check_match_exhaustiveness_expr(value);
                }
            }
            HirStmt::With { resource, body, .. } => {
                self.check_match_exhaustiveness_expr(resource);
                self.check_match_exhaustiveness_block(body);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.check_match_exhaustiveness_expr(condition);
                self.check_match_exhaustiveness_block(then_body);
                if let Some(else_body) = else_body {
                    self.check_match_exhaustiveness_block(else_body);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.check_match_exhaustiveness_expr(condition);
                }
                self.check_match_exhaustiveness_block(body);
            }
            HirStmt::For { iterable, body, .. } => {
                self.check_match_exhaustiveness_expr(iterable);
                self.check_match_exhaustiveness_block(body);
            }
            HirStmt::Match {
                value, arms, span, ..
            } => {
                self.check_match_exhaustiveness_expr(value);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.check_match_exhaustiveness_expr(guard);
                    }
                    self.check_match_exhaustiveness_block(&arm.body);
                }
                if !self.match_is_exhaustive_with_context(value, arms) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NON_EXHAUSTIVE_MATCH,
                            "match statement is not exhaustive.",
                            span.clone(),
                            "non-exhaustive match",
                        )
                        .with_cause(
                            "Supported match statements must cover `Some`/`None`, `Ok`/`Err`, all sum type variants, or include `_`.",
                        )
                        .with_fix(
                            "add_missing_arm",
                            "Add the missing variant arm or a final `_` fallback.",
                            "manual",
                        ),
                    );
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    self.check_match_exhaustiveness_expr(&arm.operation);
                    self.check_match_exhaustiveness_block(&arm.body);
                }
            }
            HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
                self.check_match_exhaustiveness_expr(expr)
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }

    fn match_is_exhaustive_with_context(&self, value: &HirExpr, arms: &[HirMatchArm]) -> bool {
        let Some(type_name) = hir_expr_type_name(value) else {
            let arm_names = arms
                .iter()
                .filter(|arm| arm.guard.is_none())
                .filter_map(|arm| arm.pattern.constructor_name().map(str::to_string))
                .collect();
            return builtin_match_is_exhaustive(&arm_names);
        };
        let patterns = arms
            .iter()
            .filter(|arm| arm.guard.is_none())
            .map(|arm| &arm.pattern)
            .collect::<Vec<_>>();
        self.patterns_cover_type(&patterns, type_name)
    }

    fn patterns_cover_type(&self, patterns: &[&MatchPattern], type_name: &str) -> bool {
        if patterns.iter().any(|pattern| {
            matches!(
                pattern,
                MatchPattern::Wildcard(_) | MatchPattern::Binding { .. }
            )
        }) {
            return true;
        }
        let root = self.resolve_type_alias(type_root_name(type_name));
        if root == "Bool" {
            let bool_literals = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Literal {
                        value: crate::syntax::ast::MatchLiteral::Bool(value),
                        ..
                    } => Some(*value),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            return bool_literals.contains(&true) && bool_literals.contains(&false);
        }
        if root == "Option" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let some_has_irrefutable_payload = patterns.iter().any(|pattern| {
                matches!(pattern, MatchPattern::Variant { name, binding: None, .. } if name == "Some")
            });
            let some_patterns = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Variant { name, binding, .. } if name == "Some" => {
                        binding.as_deref()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let none_covered = patterns.iter().any(
                |pattern| matches!(pattern, MatchPattern::Variant { name, .. } if name == "None"),
            );
            let some_covered = some_has_irrefutable_payload
                || args
                    .first()
                    .is_some_and(|inner| self.patterns_cover_type(&some_patterns, inner));
            return some_covered && none_covered;
        }
        if root == "Result" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let ok_has_irrefutable_payload = patterns.iter().any(|pattern| {
                matches!(pattern, MatchPattern::Variant { name, binding: None, .. } if name == "Ok")
            });
            let err_has_irrefutable_payload = patterns.iter().any(|pattern| {
                matches!(pattern, MatchPattern::Variant { name, binding: None, .. } if name == "Err")
            });
            let ok_patterns = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Variant { name, binding, .. } if name == "Ok" => {
                        binding.as_deref()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let err_patterns = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Variant { name, binding, .. } if name == "Err" => {
                        binding.as_deref()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let ok_covered = ok_has_irrefutable_payload
                || args
                    .first()
                    .is_some_and(|inner| self.patterns_cover_type(&ok_patterns, inner));
            let err_covered = err_has_irrefutable_payload
                || args
                    .get(1)
                    .is_some_and(|inner| self.patterns_cover_type(&err_patterns, inner));
            return ok_covered && err_covered;
        }
        if let Some(variants) = self.sum_variants_for_type(root) {
            return variants.iter().all(|(variant_name, fields)| {
                let matching = patterns
                    .iter()
                    .filter(|pattern| pattern.constructor_name() == Some(variant_name.as_str()))
                    .copied()
                    .collect::<Vec<_>>();
                self.patterns_cover_constructor_fields(&matching, fields)
            });
        }
        if matches!(
            self.hir.type_kind(root),
            Some(HirTypeKind::Struct | HirTypeKind::Class)
        ) {
            let fields = self
                .hir
                .type_info(root)
                .map(|info| info.fields.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            return self.patterns_cover_constructor_fields(patterns, &fields);
        }
        let arm_names = patterns
            .iter()
            .filter_map(|pattern| pattern.constructor_name().map(str::to_string))
            .collect();
        builtin_match_is_exhaustive(&arm_names)
    }

    fn patterns_cover_constructor_fields(
        &self,
        patterns: &[&MatchPattern],
        fields: &[FieldInfo],
    ) -> bool {
        if fields.is_empty() {
            return !patterns.is_empty();
        }
        if patterns
            .iter()
            .any(|pattern| constructor_pattern_is_irrefutable(pattern))
        {
            return true;
        }
        let Some(witnesses) = self.field_witness_product(fields) else {
            return false;
        };
        witnesses.iter().all(|witness| {
            patterns
                .iter()
                .any(|pattern| self.pattern_matches_fields(pattern, witness))
        })
    }

    fn field_witness_product(
        &self,
        fields: &[FieldInfo],
    ) -> Option<Vec<Vec<(String, PatternWitness)>>> {
        const MAX_PATTERN_WITNESSES: usize = 512;
        let mut rows: Vec<Vec<(String, PatternWitness)>> = vec![Vec::new()];
        for field in fields {
            let domain = self
                .finite_type_witnesses(&field.type_name)
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            if rows.len().saturating_mul(domain.len()) > MAX_PATTERN_WITNESSES {
                return None;
            }
            let mut next = Vec::new();
            for row in &rows {
                for witness in &domain {
                    let mut row = row.clone();
                    row.push((field.name.clone(), witness.clone()));
                    next.push(row);
                }
            }
            rows = next;
        }
        Some(rows)
    }

    fn finite_type_witnesses(&self, type_name: &str) -> Option<Vec<PatternWitness>> {
        let root = self.resolve_type_alias(type_root_name(type_name));
        if root == "Bool" {
            return Some(vec![
                PatternWitness::Bool(true),
                PatternWitness::Bool(false),
            ]);
        }
        if root == "Option" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let payload = args
                .first()
                .and_then(|inner| self.finite_type_witnesses(inner))
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            let mut witnesses = vec![PatternWitness::Constructor {
                name: "None".to_string(),
                fields: Vec::new(),
            }];
            witnesses.extend(
                payload
                    .into_iter()
                    .map(|value| PatternWitness::Constructor {
                        name: "Some".to_string(),
                        fields: vec![("value".to_string(), value)],
                    }),
            );
            return Some(witnesses);
        }
        if root == "Result" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let ok_payload = args
                .first()
                .and_then(|inner| self.finite_type_witnesses(inner))
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            let err_payload = args
                .get(1)
                .and_then(|inner| self.finite_type_witnesses(inner))
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            let mut witnesses = Vec::new();
            witnesses.extend(
                ok_payload
                    .into_iter()
                    .map(|value| PatternWitness::Constructor {
                        name: "Ok".to_string(),
                        fields: vec![("value".to_string(), value)],
                    }),
            );
            witnesses.extend(
                err_payload
                    .into_iter()
                    .map(|value| PatternWitness::Constructor {
                        name: "Err".to_string(),
                        fields: vec![("value".to_string(), value)],
                    }),
            );
            return Some(witnesses);
        }
        if let Some(variants) = self.sum_variants_for_type(root) {
            let mut witnesses = Vec::new();
            for (variant_name, fields) in variants {
                let field_rows = self.field_witness_product(&fields)?;
                witnesses.extend(field_rows.into_iter().map(|fields| {
                    PatternWitness::Constructor {
                        name: variant_name.clone(),
                        fields,
                    }
                }));
            }
            return Some(witnesses);
        }
        if matches!(
            self.hir.type_kind(root),
            Some(HirTypeKind::Struct | HirTypeKind::Class)
        ) {
            let fields = self
                .hir
                .type_info(root)
                .map(|info| info.fields.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            return self.field_witness_product(&fields).map(|rows| {
                rows.into_iter()
                    .map(|fields| PatternWitness::Constructor {
                        name: root.to_string(),
                        fields,
                    })
                    .collect()
            });
        }
        None
    }

    fn pattern_matches_fields(
        &self,
        pattern: &MatchPattern,
        fields: &[(String, PatternWitness)],
    ) -> bool {
        if constructor_pattern_is_irrefutable(pattern) {
            return true;
        }
        constrained_field_patterns(pattern)
            .into_iter()
            .all(|(name, pattern)| {
                fields
                    .iter()
                    .find(|(field_name, _)| field_name == &name)
                    .is_some_and(|(_, witness)| self.pattern_matches_witness(pattern, witness))
            })
    }

    fn pattern_matches_witness(&self, pattern: &MatchPattern, witness: &PatternWitness) -> bool {
        match pattern {
            MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => true,
            MatchPattern::Literal {
                value: crate::syntax::ast::MatchLiteral::Bool(value),
                ..
            } => matches!(witness, PatternWitness::Bool(candidate) if candidate == value),
            MatchPattern::Literal { .. } => false,
            MatchPattern::Variant { name, binding, .. } => {
                let PatternWitness::Constructor {
                    name: witness_name,
                    fields,
                } = witness
                else {
                    return false;
                };
                if name != witness_name {
                    return false;
                }
                if let Some(binding) = binding {
                    if matches!(
                        binding.as_ref(),
                        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                    ) {
                        return true;
                    }
                    fields
                        .first()
                        .is_some_and(|(_, witness)| self.pattern_matches_witness(binding, witness))
                } else {
                    true
                }
            }
            MatchPattern::Struct {
                name, fields: _, ..
            } => {
                let PatternWitness::Constructor {
                    name: witness_name,
                    fields,
                } = witness
                else {
                    return false;
                };
                name == witness_name && self.pattern_matches_fields(pattern, fields)
            }
        }
    }

    fn sum_variants_for_type(&self, root: &str) -> Option<Vec<(String, Vec<FieldInfo>)>> {
        for item in &self.syntax_program.items {
            if let Item::SumType(sum) = item
                && sum.name == root
            {
                let variants = sum
                    .variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.name.clone(),
                            self.hir
                                .sum_variant_fields(&variant.name)
                                .map(<[FieldInfo]>::to_vec)
                                .unwrap_or_default(),
                        )
                    })
                    .collect();
                return Some(variants);
            }
        }
        None
    }

    fn check_match_exhaustiveness_expr(&mut self, expr: &HirExpr) {
        match expr {
            HirExpr::Binary { left, right, .. } => {
                self.check_match_exhaustiveness_expr(left);
                self.check_match_exhaustiveness_expr(right);
            }
            HirExpr::Field { base, .. } => self.check_match_exhaustiveness_expr(base),
            HirExpr::Index { base, index, .. } => {
                self.check_match_exhaustiveness_expr(base);
                self.check_match_exhaustiveness_expr(index);
            }
            HirExpr::Call { args, .. } => {
                for arg in args {
                    self.check_match_exhaustiveness_expr(&arg.value);
                }
            }
            HirExpr::Effect { value, .. }
            | HirExpr::Manage { value, .. }
            | HirExpr::Spawn { value, .. }
            | HirExpr::Await { value, .. }
            | HirExpr::Try { value, .. } => {
                self.check_match_exhaustiveness_expr(value);
            }
            HirExpr::Closure { body, .. } => self.check_match_exhaustiveness_block(body),
            HirExpr::Match {
                value, arms, span, ..
            } => {
                self.check_match_exhaustiveness_expr(value);
                for arm in arms {
                    self.check_match_exhaustiveness_block(&arm.body);
                }
                if !self.match_is_exhaustive_with_context(value, arms) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NON_EXHAUSTIVE_MATCH,
                            "match expression is not exhaustive.",
                            span.clone(),
                            "non-exhaustive match",
                        )
                        .with_cause(
                            "Supported match expressions must cover `Some`/`None`, `Ok`/`Err`, all sum type variants, or include `_`.",
                        )
                        .with_fix(
                            "add_missing_arm",
                            "Add the missing variant arm or a final `_` fallback.",
                            "manual",
                        ),
                    );
                }
            }
            HirExpr::ObjectLiteral { .. }
            | HirExpr::MapLiteral { .. }
            | HirExpr::ArrayLiteral { .. }
            | HirExpr::Ident { .. }
            | HirExpr::Number { .. }
            | HirExpr::String { .. }
            | HirExpr::Unknown(_) => {}
        }
    }

    fn check_signature_explicitness(&mut self) {
        let items = self.syntax_program.items.clone();
        let protocol_names = self
            .syntax_program
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect::<HashSet<_>>();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            if function.return_ty.is_none() {
                self.diagnostics.push(
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

            let is_qualified_method = function.name.contains('.');
            let is_protocol_method = function
                .name
                .split_once('.')
                .is_some_and(|(namespace, _)| protocol_names.contains(namespace));
            for (index, param) in function.params.iter().enumerate() {
                if param.name == "self" && (!is_qualified_method || index != 0) {
                    self.invalid_self_parameter_diagnostic(
                        function,
                        param,
                        "`self` may only be the first parameter of a qualified method signature.",
                    );
                }
                if param.ty.name.is_empty() {
                    self.diagnostics.push(
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
                if param.effect.is_none() && param.ty.name == "share" {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::REMOVED_SHARE_EFFECT,
                            format!(
                                "parameter `{}` in `{}` uses removed `share` data effect.",
                                param.name, function.name
                            ),
                            param.ty.span.clone(),
                            "removed share data effect",
                        )
                        .with_cause("RSScript v0.6 has exactly three data effects: `read`, `mut`, and `take`.")
                        .with_fix(
                            "replace_share_effect",
                            format!(
                                "Use `{}: read T` and add `effects(retains({}))` if the function retains it.",
                                param.name, param.name
                            ),
                            "manual",
                        ),
                    );
                }
                if param.effect.is_none()
                    && !param.ty.name.is_empty()
                    && param.ty.name != "share"
                    && !type_ref_is_noescape(&param.ty)
                    && !type_ref_is_owned(&param.ty)
                    && !type_ref_is_closure_effect_exempt(&param.ty)
                    && !type_ref_has_surface_reference(&param.ty, self.tokens)
                    && !type_ref_contains_name(&param.ty, "Fd")
                    && !is_copy_type(&param.ty)
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::MISSING_PARAMETER_EFFECT,
                            format!(
                                "parameter `{}` in `{}` must declare `read`, `mut`, or `take`.",
                                param.name, function.name
                            ),
                            param.span.clone(),
                            "missing parameter effect",
                        )
                        .with_cause("Non-Copy parameters must expose their data effect in the function signature.")
                        .with_fix(
                            "add_parameter_effect",
                            format!("Write `{}: read {}` or another explicit effect.", param.name, type_ref_name(&param.ty)),
                            "manual",
                        ),
                    );
                }
            }

            if is_protocol_method {
                match function.params.first() {
                    Some(param)
                        if param.name == "self"
                            && param.ty.name == "Self"
                            && param.effect.is_some() => {}
                    Some(param) => self.invalid_self_parameter_diagnostic(
                        function,
                        param,
                        "Protocol methods must declare `self: read|mut|take Self` as their first parameter.",
                    ),
                    None => self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_SELF_PARAMETER,
                            format!(
                                "protocol method `{}` must declare an explicit `self` parameter.",
                                function.name
                            ),
                            function.span.clone(),
                            "missing protocol self parameter",
                        )
                        .with_cause("Protocol calls are explicit capability calls, so the receiver must be review-visible as `self: read|mut|take Self`.")
                        .with_fix(
                            "add_protocol_self",
                            "Add `self: read Self`, `self: mut Self`, or `self: take Self` as the first parameter.",
                            "manual",
                        ),
                    ),
                }
            }

            for effect in &function.effects {
                let effect_name = effect_name(effect);
                if effect_name == "fresh" {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_EFFECT,
                            format!(
                                "`fresh` is not a valid `effects(...)` item in `{}`.",
                                function.name
                            ),
                            function.span.clone(),
                            "fresh is a return marker",
                        )
                        .with_cause(
                            "`fresh` is a return contract, not a side effect or runtime guarantee.",
                        )
                        .with_fix(
                            "move_fresh_to_return_type",
                            "Write `-> fresh T` or `-> Result<fresh T, E>` instead.",
                            "manual",
                        ),
                    );
                    continue;
                }
                if let Some(replacement) = removed_runtime_effect_replacement(effect_name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::REMOVED_RUNTIME_EFFECT,
                            format!(
                                "`{effect_name}` is not a valid RSScript v0.6 effect in `{}`.",
                                function.name
                            ),
                            function.span.clone(),
                            "removed runtime effect",
                        )
                        .with_cause("v0.6 uses reductive guarantees such as `no_panic`, `noalloc`, `no_block`, and `pure`.")
                        .with_fix(
                            "replace_removed_effect",
                            replacement,
                            "manual",
                        ),
                    );
                    continue;
                }
                let valid = effect_name == "no_panic"
                    || effect_name == "noalloc"
                    || effect_name == "no_block"
                    || effect_name == "pure"
                    || effect_name == "unsafe"
                    || effect_name == "native"
                    || effect_name == "parallel"
                    || matches!(effect, EffectDecl::Retains(_));
                if !valid {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_EFFECT,
                            format!("unknown effect `{effect_name}` in `{}`.", function.name),
                            function.span.clone(),
                            "unknown effect",
                        )
                        .with_cause("Effects are review-visible semantic contracts; unknown effect names cannot be interpreted safely.")
                        .with_fix(
                            "fix_unknown_effect",
                            "Use a recognized effect such as `pure`, `no_panic`, `noalloc`, `no_block`, `native`, `unsafe`, `parallel`, or `retains(param)`.",
                            "manual",
                        ),
                    );
                }
            }

            if function.is_native && !function_has_effect(function, "native") {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::FEATURE_VIOLATION,
                        format!(
                            "`native fn {}` must declare `effects(native)`.",
                            function.name
                        ),
                        function.span.clone(),
                        "native boundary missing effect",
                    )
                    .with_cause(
                        "`native fn` is an implementation boundary, and v0.6 requires that boundary to appear in the effect list for review maps and semantic diffs.",
                    )
                    .with_fix(
                        "add_native_effect",
                        "Add `effects(native)` to the native function declaration.",
                        "manual",
                    ),
                );
            }

            let param_names: HashSet<&str> = function
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect();
            for effect in &function.effects {
                let EffectDecl::Retains(param) = effect else {
                    continue;
                };
                if !param_names.contains(param.as_str()) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_RETAINED_PARAMETER,
                            format!(
                                "`{}` declares `effects(retains({param}))`, but `{param}` is not a parameter.",
                                function.name
                            ),
                            function.span.clone(),
                            "unknown retained parameter",
                        )
                        .with_cause("Retention effects must name a parameter from the same function signature.")
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
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_RETAINED_PARAMETER,
                            format!(
                                "`{}` declares `effects(retains({param}))`, but `{param}` is Copy.",
                                function.name
                            ),
                            function_param.span.clone(),
                            "Copy parameter cannot be retained",
                        )
                        .with_cause(
                            "`retains(x)` marks a managed retention boundary. Copy values have no managed handle to retain.",
                        )
                        .with_fix(
                            "remove_copy_retains",
                            format!("Remove `effects(retains({param}))`."),
                            "manual",
                        ),
                    );
                    continue;
                }
                if function.params.iter().any(|function_param| {
                    function_param.name == *param && type_ref_is_noescape(&function_param.ty)
                }) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NOESCAPE_CALLBACK_ESCAPE,
                            format!(
                                "`{}` cannot retain noescape callback parameter `{param}`.",
                                function.name
                            ),
                            function.span.clone(),
                            "noescape callback escapes",
                        )
                        .with_cause(
                            "`noescape Fn()` parameters may be called or forwarded to another noescape parameter, but they cannot be retained after return.",
                        )
                        .with_fix(
                            "remove_noescape_retention",
                            format!("Remove `effects(retains({param}))`, or use an ordinary managed callback type."),
                            "manual",
                        ),
                    );
                }
            }

            if function
                .effects
                .iter()
                .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "pure"))
            {
                if let Some(return_ty) = &function.return_ty
                    && let Some(resource_name) = self
                        .resource_return_type_name(return_ty)
                        .map(str::to_string)
                {
                    self.pure_resource_return_diagnostic(
                        &function.name,
                        return_ty.span.clone(),
                        &resource_name,
                    );
                }

                for param in function.params.iter().filter(|param| {
                    matches!(param.effect, Some(DataEffect::Mut | DataEffect::Take))
                }) {
                    let (label, cause, fix) = match param.effect {
                        Some(DataEffect::Take) => (
                            "taking parameter in pure function",
                            "A `pure` function must not consume values or change ownership boundaries.",
                            "Remove `pure`, or change the parameter to `read` if the function only observes it.",
                        ),
                        _ => (
                            "mutable parameter in pure function",
                            "A `pure` function must not mutate reachable managed state.",
                            "Remove `pure`, or change the parameter to `read` if the function does not mutate it.",
                        ),
                    };
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_PURE_EFFECT,
                            format!(
                                "`{}` is declared pure but parameter `{}` uses `{}`.",
                                function.name,
                                param.name,
                                param.effect.map_or("unknown", data_effect_name)
                            ),
                            param.span.clone(),
                            label,
                        )
                        .with_cause(cause)
                        .with_fix("remove_pure_or_use_read", fix, "manual"),
                    );
                }

                for effect in function
                    .effects
                    .iter()
                    .filter(|effect| matches!(effect, EffectDecl::Retains(_)))
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_PURE_EFFECT,
                            format!(
                                "`{}` is declared pure but also retains a parameter.",
                                function.name
                            ),
                            function.span.clone(),
                            "retention in pure function",
                        )
                        .with_cause("A `pure` function must not retain parameters after returning.")
                        .with_fix(
                            "remove_pure_or_retains",
                            format!("Remove `pure` or remove `{}`.", effect_display(effect)),
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    fn invalid_self_parameter_diagnostic(
        &mut self,
        function: &FunctionDecl,
        param: &Param,
        cause: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_SELF_PARAMETER,
                format!("invalid `self` parameter in `{}`.", function.name),
                param.span.clone(),
                "invalid self parameter",
            )
            .with_cause(cause)
            .with_fix(
                "fix_self_parameter",
                "Use a different parameter name, or make this the first parameter of an explicit method/protocol signature.",
                "manual",
            ),
        );
    }

    fn check_generic_constraints(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    let bounds = generic_bounds(&decl.type_params);
                    if decl.kind == TypeKind::Resource {
                        for param in &decl.type_params {
                            if param.bound.is_none() {
                                self.generic_resource_argument_diagnostic(
                                    &param.name,
                                    &param.name,
                                    &param.span,
                                    "resource type parameters must declare an explicit bound.",
                                );
                            }
                        }
                        for field in &decl.fields {
                            self.check_resource_type_param_field(&field.ty, &bounds, false);
                        }
                    }
                    for field in &decl.fields {
                        self.check_generic_resource_pool_type_ref(&field.ty, &bounds);
                    }
                }
                Item::Function(function) => {
                    let bounds = generic_bounds(&function.type_params);
                    for param in &function.params {
                        self.check_generic_resource_pool_type_ref(&param.ty, &bounds);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_generic_resource_pool_type_ref(return_ty, &bounds);
                        if function.returns_fresh {
                            self.check_fresh_generic_return_bound(
                                &function.name,
                                return_ty,
                                &bounds,
                            );
                        }
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    fn check_protocol_contracts(&mut self) {
        let protocol_names = self.visible_protocol_names();
        let items = self.visible_protocol_items();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for param in &decl.type_params {
                        self.check_protocol_bound(param, &protocol_names);
                    }
                }
                Item::Function(function) => {
                    if function_body_belongs_to_protocol(function, &protocol_names) {
                        self.unsupported_syntax(
                            function.span.clone(),
                            "unsupported protocol method body",
                            "Protocols are effect-carrying capability contracts in v0.6. Protocol methods are bodyless signatures; default method bodies are not part of the RSScript protocol model.",
                        );
                    }
                    if function.default_impl_marker
                        && !function_belongs_to_protocol(function, &protocol_names)
                    {
                        self.unsupported_syntax(
                            function.span.clone(),
                            "unsupported default implementation marker",
                            "`= _` is reserved for protocol method contracts so defaulted protocol behavior is review-visible.",
                        );
                    }
                    for param in &function.type_params {
                        self.check_protocol_bound(param, &protocol_names);
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }

        let protocol_impls = self.syntax_program.protocol_impls.clone();
        for protocol_impl in &protocol_impls {
            if !protocol_names.contains(&protocol_impl.protocol) {
                self.unknown_protocol_diagnostic(&protocol_impl.protocol, &protocol_impl.span);
                continue;
            }
            if self.hir.type_info(&protocol_impl.type_name).is_none()
                && !is_builtin_type_name(&protocol_impl.type_name)
            {
                self.unknown_type_name_diagnostic(&protocol_impl.type_name, &protocol_impl.span);
            }
            let protocol_methods = protocol_method_names(&items, &protocol_impl.protocol);
            let mapped_methods = protocol_impl
                .mappings
                .iter()
                .map(|mapping| mapping.method.clone())
                .collect::<HashSet<_>>();
            for method in &protocol_methods {
                if !mapped_methods.contains(method) {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        method,
                        &protocol_impl.span,
                        "missing protocol method mapping",
                        format!(
                            "`{}` must map protocol method `{method}` to a concrete function.",
                            protocol_impl.type_name
                        ),
                    );
                }
            }

            for mapping in &protocol_impl.mappings {
                let Some(protocol_signature) = self
                    .hir
                    .resolve_function(Some(&protocol_impl.protocol), &mapping.method)
                else {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "unknown protocol method",
                        format!(
                            "`{}` does not declare method `{}`.",
                            protocol_impl.protocol, mapping.method
                        ),
                    );
                    continue;
                };
                let (target_namespace, target_name) = split_qualified_name(&mapping.target);
                let Some(target_signature) = self
                    .hir
                    .resolve_function(target_namespace.as_deref(), target_name)
                else {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "unknown protocol implementation target",
                        format!(
                            "Mapped target `{}` must resolve to a concrete function.",
                            mapping.target
                        ),
                    );
                    continue;
                };
                if let Some(reason) = protocol_signature_mismatch(
                    protocol_signature,
                    target_signature,
                    &protocol_impl.type_name,
                ) {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "protocol implementation signature mismatch",
                        reason,
                    );
                }
            }
        }
    }

    fn visible_protocol_names(&self) -> HashSet<String> {
        self.interface_programs
            .iter()
            .flat_map(|program| program.protocols.iter())
            .chain(self.syntax_program.protocols.iter())
            .map(|protocol| protocol.name.clone())
            .collect()
    }

    pub(crate) fn protocol_name_is_visible(&self, name: &str) -> bool {
        self.interface_programs
            .iter()
            .flat_map(|program| program.protocols.iter())
            .chain(self.syntax_program.protocols.iter())
            .any(|protocol| protocol.name == name)
    }

    fn visible_protocol_items(&self) -> Vec<Item> {
        self.interface_programs
            .iter()
            .flat_map(|program| program.items.iter().cloned())
            .chain(self.syntax_program.items.iter().cloned())
            .collect()
    }

    fn check_protocol_bound(&mut self, param: &GenericParam, protocol_names: &HashSet<String>) {
        let Some(GenericBound::Protocol(protocol)) = &param.bound else {
            return;
        };
        if !protocol_names.contains(protocol) {
            self.unknown_protocol_diagnostic(protocol, &param.span);
        }
    }

    fn check_unknown_types(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    let generic_params = decl
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<HashSet<_>>();
                    for field in &decl.fields {
                        self.check_unknown_type_ref(&field.ty, &generic_params);
                    }
                }
                Item::Function(function) => {
                    let generic_params = function
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<HashSet<_>>();
                    for param in &function.params {
                        self.check_unknown_type_ref(&param.ty, &generic_params);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_unknown_type_ref(return_ty, &generic_params);
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    fn check_unknown_type_ref(&mut self, ty: &TypeRef, generic_params: &HashSet<&str>) {
        if ty.name == "Capability" {
            self.check_capability_type_ref(ty, generic_params);
            for param in &ty.fn_params {
                self.check_unknown_type_ref(param, generic_params);
            }
            if let Some(return_ty) = &ty.fn_return {
                self.check_unknown_type_ref(return_ty, generic_params);
            }
            return;
        }
        // Type aliases are known types
        if self.type_aliases.contains_key(&ty.name) {
            // Valid - it's a type alias
        } else if !known_type_ref(ty, generic_params, &self.hir) {
            self.unknown_type_diagnostic(ty);
        }
        for arg in &ty.args {
            self.check_unknown_type_ref(arg, generic_params);
        }
        for param in &ty.fn_params {
            self.check_unknown_type_ref(param, generic_params);
        }
        if let Some(return_ty) = &ty.fn_return {
            self.check_unknown_type_ref(return_ty, generic_params);
        }
    }

    fn check_capability_type_ref(&mut self, ty: &TypeRef, generic_params: &HashSet<&str>) {
        if ty.args.len() != 1 {
            self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
            return;
        }
        let protocol = &ty.args[0];
        if !protocol.args.is_empty()
            || !protocol.fn_params.is_empty()
            || protocol.fn_return.is_some()
            || protocol.is_fresh
            || protocol.is_noescape
            || protocol.is_owned
        {
            self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
            return;
        }
        if generic_params.contains(protocol.name.as_str()) {
            return;
        }
        if !self.protocol_name_is_visible(&protocol.name) {
            self.unknown_protocol_diagnostic(&protocol.name, &protocol.span);
        }
    }

    fn check_unknown_fields(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let Some(body) = self.hir.function_body(&function.name).cloned() else {
                continue;
            };
            for access in &body.field_accesses {
                let Some(base_type) = &access.base_type else {
                    continue;
                };
                let Some(type_info) = self.hir.type_info(base_type) else {
                    continue;
                };
                if !type_info.fields.contains_key(&access.name) {
                    self.unknown_field_diagnostic(&access.name, base_type, &access.span);
                }
            }
        }
    }

    fn check_unknown_bindings(&mut self) {
        // Collect top-level const names and sum type variant names as globally visible bindings
        let mut global_names: HashSet<String> = HashSet::new();
        for item in &self.syntax_program.items {
            match item {
                Item::Const(decl) => {
                    global_names.insert(decl.name.clone());
                }
                Item::SumType(sum) => {
                    for variant in &sum.variants {
                        global_names.insert(variant.name.clone());
                    }
                }
                _ => {}
            }
        }
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let Some(block) = self
                .hir
                .function_body(&function.name)
                .and_then(|body| body.block.clone())
            else {
                continue;
            };
            let mut visible: HashSet<String> = function
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect();
            visible.extend(global_names.iter().cloned());
            self.check_unknown_bindings_in_block(&block, &mut visible);
        }
    }

    fn check_unknown_bindings_in_block(&mut self, block: &HirBlock, visible: &mut HashSet<String>) {
        for statement in &block.statements {
            self.check_unknown_bindings_in_stmt(statement, visible);
        }
    }

    fn check_unknown_bindings_in_stmt(
        &mut self,
        statement: &HirStmt,
        visible: &mut HashSet<String>,
    ) {
        match statement {
            HirStmt::Let { name, value, .. } => {
                if let Some(value) = value {
                    self.check_unknown_bindings_in_expr(value, visible);
                }
                visible.insert(name.clone());
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.check_unknown_bindings_in_expr(value, visible);
                }
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(resource, visible);
                let mut body_visible = visible.clone();
                body_visible.insert(binding.clone());
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(condition, visible);
                let mut then_visible = visible.clone();
                self.check_unknown_bindings_in_block(then_body, &mut then_visible);
                if let Some(else_body) = else_body {
                    let mut else_visible = visible.clone();
                    self.check_unknown_bindings_in_block(else_body, &mut else_visible);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.check_unknown_bindings_in_expr(condition, visible);
                }
                let mut body_visible = visible.clone();
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(iterable, visible);
                let mut body_visible = visible.clone();
                body_visible.insert(binding.clone());
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::Match { value, arms, .. } => {
                self.check_unknown_bindings_in_expr(value, visible);
                for arm in arms {
                    let mut arm_visible = visible.clone();
                    for binding in arm.pattern.binding_names() {
                        arm_visible.insert(binding.to_string());
                    }
                    if let Some(guard) = &arm.guard {
                        self.check_unknown_bindings_in_expr(guard, &arm_visible);
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    self.check_unknown_bindings_in_expr(&arm.operation, visible);
                    let mut arm_visible = visible.clone();
                    if arm.binding != "_" {
                        arm_visible.insert(arm.binding.clone());
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
                self.check_unknown_bindings_in_expr(value, visible)
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }

    fn check_unknown_bindings_in_expr(&mut self, expr: &HirExpr, visible: &HashSet<String>) {
        match expr {
            HirExpr::Ident { name, span, .. } => {
                if !visible.contains(name) && !builtin_value_ident(name) {
                    self.unknown_binding_diagnostic(name, span);
                }
            }
            HirExpr::Binary { left, right, .. } => {
                self.check_unknown_bindings_in_expr(left, visible);
                self.check_unknown_bindings_in_expr(right, visible);
            }
            HirExpr::Field { base, .. } => self.check_unknown_bindings_in_expr(base, visible),
            HirExpr::Index { base, index, .. } => {
                self.check_unknown_bindings_in_expr(base, visible);
                self.check_unknown_bindings_in_expr(index, visible);
            }
            HirExpr::Call { args, .. } => {
                for arg in args {
                    self.check_unknown_bindings_in_expr(&arg.value, visible);
                }
            }
            HirExpr::Effect { value, .. }
            | HirExpr::Manage { value, .. }
            | HirExpr::Spawn { value, .. }
            | HirExpr::Await { value, .. }
            | HirExpr::Try { value, .. } => self.check_unknown_bindings_in_expr(value, visible),
            HirExpr::Closure { params, body, .. } => {
                let mut closure_visible = visible.clone();
                closure_visible.extend(params.iter().cloned());
                self.check_unknown_bindings_in_block(body, &mut closure_visible);
            }
            HirExpr::Match { value, arms, .. } => {
                self.check_unknown_bindings_in_expr(value, visible);
                for arm in arms {
                    let mut arm_visible = visible.clone();
                    for binding in arm.pattern.binding_names() {
                        arm_visible.insert(binding.to_string());
                    }
                    if let Some(guard) = &arm.guard {
                        self.check_unknown_bindings_in_expr(guard, &arm_visible);
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirExpr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_unknown_bindings_in_expr(&entry.key, visible);
                    self.check_unknown_bindings_in_expr(&entry.value, visible);
                }
            }
            HirExpr::ObjectLiteral { .. }
            | HirExpr::ArrayLiteral { .. }
            | HirExpr::Number { .. }
            | HirExpr::String { .. }
            | HirExpr::Unknown(_) => {}
        }
    }

    fn check_fresh_generic_return_bound(
        &mut self,
        function_name: &str,
        return_ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        let target = fresh_return_target_type(return_ty);
        // A protocol method's implicit `Self` parameter is bound `Managed`, which
        // still admits a `fresh Self` return: managed structs/sums are freshly
        // ownable, and the per-instantiation derive (`derives(Clone)`) is checked
        // at the use site. A `fresh Self` from a value scalar is impossible
        // because scalars do not satisfy the protocol's `Managed` `Self` bound.
        let bound = bounds.get(&target.name).and_then(Option::as_ref);
        let fresh_bound_ok = matches!(bound, Some(GenericBound::Struct))
            || (target.name == "Self" && matches!(bound, Some(GenericBound::Managed)));
        if bounds.contains_key(&target.name) && !fresh_bound_ok {
            self.diagnostics.push(
                Diagnostic::error(
                    code::INVALID_FRESH_RETURN_TYPE,
                    format!(
                        "function `{function_name}` returns `fresh {}` but `{}` is not bounded by `Struct`.",
                        target.name, target.name
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

    fn check_generic_resource_pool_type_ref(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        if ty.name == "ResourcePool"
            && let Some(arg) = ty.args.first()
            && let Some(bound) = bounds.get(&arg.name)
            && bound.as_ref() != Some(&GenericBound::Resource)
        {
            self.invalid_resource_pool_type_diagnostic(
                format!(
                    "ResourcePool<{}> requires `{}` to be explicitly bounded by Resource.",
                    arg.name, arg.name
                ),
                arg.span.clone(),
            );
        }
        for arg in &ty.args {
            self.check_generic_resource_pool_type_ref(arg, bounds);
        }
    }

    fn check_resource_type_param_field(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
        in_resource_pool: bool,
    ) {
        let next_in_resource_pool =
            in_resource_pool || self.is_approved_resource_container(&ty.name);
        if !next_in_resource_pool
            && bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource)
        {
            self.generic_resource_argument_diagnostic(
                &ty.name,
                &ty.name,
                &ty.span,
                "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
            );
        }
        for arg in &ty.args {
            self.check_resource_type_param_field(arg, bounds, next_in_resource_pool);
        }
    }

    fn check_try_operator_result_returns(&mut self) {
        for (index, item) in self.syntax_program.items.iter().enumerate() {
            let Item::Function(function) = item else {
                continue;
            };
            if function
                .return_ty
                .as_ref()
                .is_some_and(|return_ty| return_ty.name == "Result")
            {
                continue;
            }

            let start = self
                .tokens
                .iter()
                .position(|token| token.span == function.span)
                .unwrap_or(0);
            let end = self
                .syntax_program
                .items
                .iter()
                .skip(index + 1)
                .map(item_span)
                .find_map(|span| self.tokens.iter().position(|token| token.span == *span))
                .unwrap_or(self.tokens.len());

            for token in &self.tokens[start..end] {
                if token.symbol("?") {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_TRY_OPERATOR,
                            format!(
                                "`?` in `{}` requires the function to return `Result<T, E>`.",
                                function.name
                            ),
                            token.span.clone(),
                            "invalid try operator",
                        )
                        .with_cause("RSScript represents recoverable failure in explicit `Result` return types.")
                        .with_fix(
                            "return_result_or_handle_error",
                            "Change the return type to `Result<..., E>` or handle the error explicitly.",
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    fn check_runtime_guarantee_bodies(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            for guarantee in RuntimeGuarantee::ALL {
                if function_has_effect(function, guarantee.effect_name()) {
                    self.check_runtime_guarantee_block(guarantee, &function.name, &function.body);
                }
            }
        }
    }

    fn check_runtime_guarantee_block(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        block: &Block,
    ) {
        for statement in &block.statements {
            self.check_runtime_guarantee_stmt(guarantee, function_name, statement);
        }
    }

    fn check_runtime_guarantee_stmt(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        statement: &Stmt,
    ) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_runtime_guarantee_expr(guarantee, function_name, value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_runtime_guarantee_expr(guarantee, function_name, value);
                }
            }
            Stmt::Assign(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.target);
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
            }
            Stmt::Expr(value) => self.check_runtime_guarantee_expr(guarantee, function_name, value),
            Stmt::With(stmt) => {
                if guarantee == RuntimeGuarantee::Pure {
                    self.pure_with_resource_diagnostic(function_name, &stmt.span);
                }
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.resource);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.condition);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_runtime_guarantee_block(guarantee, function_name, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_runtime_guarantee_expr(guarantee, function_name, condition);
                }
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.iterable);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &arm.operation);
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
                for arm in &stmt.arms {
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.else_body);
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

    fn check_runtime_guarantee_expr(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        expr: &Expr,
    ) {
        match expr {
            Expr::Call {
                callee, args, span, ..
            } => {
                match self.hir.resolve_call(callee) {
                    CallResolution::Resolved { signature, kind } => {
                        if matches!(kind, ResolvedCalleeKind::Constructor { .. }) {
                            if guarantee == RuntimeGuarantee::Noalloc {
                                self.noalloc_allocation_diagnostic(
                                    function_name,
                                    span,
                                    format!(
                                        "constructor `{}` creates a new value.",
                                        callee_display(callee)
                                    ),
                                );
                            }
                        } else if !signature
                            .effects
                            .iter()
                            .any(|effect| effect == guarantee.effect_name())
                        {
                            self.runtime_guarantee_call_diagnostic(
                                guarantee,
                                function_name,
                                callee,
                                span,
                            );
                        }
                    }
                    CallResolution::EnumVariant
                    | CallResolution::Ambiguous { .. }
                    | CallResolution::Unknown => {}
                }
                for arg in args {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &arg.value);
                }
            }
            Expr::Manage { value, span } => {
                if guarantee == RuntimeGuarantee::Noalloc {
                    self.noalloc_allocation_diagnostic(
                        function_name,
                        span,
                        "`manage` may allocate while migrating a local graph.".to_string(),
                    );
                }
                if guarantee == RuntimeGuarantee::Pure {
                    self.pure_manage_diagnostic(function_name, span);
                }
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
            }
            Expr::Effect { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
            }
            Expr::Binary { left, right, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, left);
                self.check_runtime_guarantee_expr(guarantee, function_name, right);
            }
            Expr::Field { base, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, base);
            }
            Expr::Index { base, index, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, base);
                self.check_runtime_guarantee_expr(guarantee, function_name, index);
            }
            Expr::Closure { body, .. } => {
                self.check_runtime_guarantee_block(guarantee, function_name, body);
            }
            Expr::Match { value, arms, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
                for arm in arms {
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &entry.key);
                    self.check_runtime_guarantee_expr(guarantee, function_name, &entry.value);
                }
            }
            Expr::ObjectLiteral { .. }
            | Expr::ArrayLiteral { .. }
            | Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    fn check_duplicate_declarations(&mut self) {
        for duplicate in self.hir.duplicate_symbols() {
            self.diagnostics.push(
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
                ),
            );
        }
    }

    /// A type allowed to hold `T: Resource` directly. For now this is only the
    /// built-in `ResourcePool`: it owns the lease/return/discard machinery that
    /// keeps resource access scoped. A user-facing extension is deferred — a plain
    /// marker `impl` would be a self-service whitelist that bypasses `RS0701` with
    /// none of those guarantees, so any extension must be a compiler-recognized
    /// declaration with explicit approval conditions, not an open `impl`.
    fn is_approved_resource_container(&self, type_name: &str) -> bool {
        type_name == "ResourcePool"
    }

    fn check_resource_fields(&mut self) {
        for item in &self.syntax_program.items {
            let Item::Type(decl) = item else {
                continue;
            };
            if self.hir.type_kind(&decl.name) == Some(HirTypeKind::Resource) {
                continue;
            }
            for field in &decl.fields {
                if self.hir.type_kind(&field.ty.name) == Some(HirTypeKind::Resource) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::RESOURCE_FIELD,
                            format!("resource `{}` cannot be stored in `{}`.", field.ty.name, decl.name),
                            field.span.clone(),
                            "resource field",
                        )
                        .with_cause("Resources must be used through `with` or approved resource containers.")
                        .with_fix("use_with", "Use `with` or `ResourcePool<T: Resource>` instead.", "manual"),
                    );
                }
            }
        }
    }

    fn check_fd_surface(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Function(function) => {
                    if function.is_native {
                        continue;
                    }
                    for param in &function.params {
                        if type_ref_contains_name(&param.ty, "Fd") {
                            self.fd_surface_diagnostic(
                                param.ty.span.clone(),
                                "`Fd` parameter outside native boundary",
                                "Use a `resource` type such as `File` instead of exposing raw descriptor handles.",
                            );
                        }
                    }
                    if let Some(return_ty) = &function.return_ty
                        && type_ref_contains_name(return_ty, "Fd")
                    {
                        self.fd_surface_diagnostic(
                            return_ty.span.clone(),
                            "`Fd` return outside native boundary",
                            "Return a `resource` type such as `File` instead of exposing raw descriptor handles.",
                        );
                    }
                }
                Item::Type(decl) => {
                    if decl.kind == TypeKind::Resource {
                        continue;
                    }
                    for field in &decl.fields {
                        if type_ref_contains_name(&field.ty, "Fd") {
                            self.fd_surface_diagnostic(
                                field.ty.span.clone(),
                                "`Fd` field outside resource internals",
                                "Use a `resource` field wrapper or a non-Fd public value type.",
                            );
                        }
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    fn check_weak_fields(&mut self) {
        for item in &self.syntax_program.items {
            let Item::Type(decl) = item else {
                continue;
            };
            for field in &decl.fields {
                if !field.is_weak {
                    continue;
                }
                if self.hir.type_kind(&field.ty.name) != Some(HirTypeKind::Class) {
                    self.diagnostics.push(
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
                        ),
                    );
                }
            }
        }
    }

    fn check_resource_pool_type_arguments(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for field in &decl.fields {
                        self.check_resource_pool_type_ref(&field.ty);
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_pool_type_ref(&param.ty);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_pool_type_ref(return_ty);
                    }
                    self.check_resource_pool_calls_in_block(&function.body);
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    fn check_resource_generic_arguments(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for field in &decl.fields {
                        self.check_resource_generic_type_ref(
                            &field.ty,
                            ResourceGenericContext::Ordinary,
                        );
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_generic_type_ref(
                            &param.ty,
                            ResourceGenericContext::Ordinary,
                        );
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_generic_type_ref(
                            return_ty,
                            ResourceGenericContext::Return,
                        );
                    }
                    self.check_resource_generic_calls_in_block(&function.body);
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    fn check_resource_pool_type_ref(&mut self, ty: &TypeRef) {
        if ty.name == "ResourcePool" {
            match ty.args.first() {
                Some(arg) => self.check_resource_pool_arg(&arg.name, &arg.span),
                None => self.invalid_resource_pool_type_diagnostic(
                    "ResourcePool must declare a resource type argument.",
                    ty.span.clone(),
                ),
            }
        }
        for arg in &ty.args {
            self.check_resource_pool_type_ref(arg);
        }
    }

    fn check_resource_pool_calls_in_block(&mut self, block: &crate::syntax::ast::Block) {
        for statement in &block.statements {
            self.check_resource_pool_calls_in_stmt(statement);
        }
    }

    fn check_resource_generic_type_ref(&mut self, ty: &TypeRef, context: ResourceGenericContext) {
        if ty.name != "ResourcePool" {
            for (index, arg) in ty.args.iter().enumerate() {
                if self.hir.type_kind(&arg.name) == Some(HirTypeKind::Resource)
                    && !resource_result_return_arg_allowed(ty, index, context)
                {
                    self.resource_generic_argument_diagnostic(&ty.name, &arg.name, &arg.span);
                }
            }
        }
        for (index, arg) in ty.args.iter().enumerate() {
            if resource_result_return_arg_allowed(ty, index, context) {
                continue;
            }
            self.check_resource_generic_type_ref(arg, ResourceGenericContext::Ordinary);
        }
    }

    fn check_resource_pool_calls_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_pool_calls_in_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_pool_calls_in_expr(value);
                }
            }
            Stmt::Assign(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.target);
                self.check_resource_pool_calls_in_expr(&stmt.value);
            }
            Stmt::Expr(value) => self.check_resource_pool_calls_in_expr(value),
            Stmt::With(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.resource);
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.condition);
                self.check_resource_pool_calls_in_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_resource_pool_calls_in_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_resource_pool_calls_in_expr(condition);
                }
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.iterable);
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_resource_pool_calls_in_expr(&arm.operation);
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.value);
                self.check_resource_pool_calls_in_block(&stmt.else_body);
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

    fn check_resource_pool_calls_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                if let Callee::Qualified { namespace, name } = callee
                    && namespace == "ResourcePool"
                    && name == "new"
                {
                    self.invalid_resource_pool_type_diagnostic(
                        "ResourcePool.new must be called as ResourcePool<T>.new with resource T.",
                        span.clone(),
                    );
                } else if let Callee::Qualified { namespace, .. } = callee
                    && let Some(arg) = resource_pool_namespace_arg(namespace)
                {
                    self.check_resource_pool_arg(arg, span);
                }
                for arg in args {
                    self.check_resource_pool_calls_in_expr(&arg.value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_resource_pool_calls_in_expr(left);
                self.check_resource_pool_calls_in_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_resource_pool_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_pool_calls_in_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_resource_pool_calls_in_expr(base);
                self.check_resource_pool_calls_in_expr(index);
            }
            Expr::Closure { body, .. } => self.check_resource_pool_calls_in_block(body),
            Expr::Match { value, arms, .. } => {
                self.check_resource_pool_calls_in_expr(value);
                for arm in arms {
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_resource_pool_calls_in_expr(&entry.key);
                    self.check_resource_pool_calls_in_expr(&entry.value);
                }
            }
            Expr::ObjectLiteral { .. }
            | Expr::ArrayLiteral { .. }
            | Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    fn check_resource_generic_calls_in_block(&mut self, block: &crate::syntax::ast::Block) {
        for statement in &block.statements {
            self.check_resource_generic_calls_in_stmt(statement);
        }
    }

    fn check_resource_generic_calls_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_generic_calls_in_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_generic_calls_in_expr(value);
                }
            }
            Stmt::Assign(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.target);
                self.check_resource_generic_calls_in_expr(&stmt.value);
            }
            Stmt::Expr(value) => self.check_resource_generic_calls_in_expr(value),
            Stmt::With(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.resource);
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.condition);
                self.check_resource_generic_calls_in_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_resource_generic_calls_in_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_resource_generic_calls_in_expr(condition);
                }
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.iterable);
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::TaskGroup(stmt) => {
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.check_resource_generic_calls_in_expr(&arm.operation);
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Stmt::Match(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.value);
                self.check_resource_generic_calls_in_block(&stmt.else_body);
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

    fn check_resource_generic_calls_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                if let Callee::Qualified { namespace, .. } = callee
                    && let Some((root, args)) = generic_namespace_args(namespace)
                    && !self.is_approved_resource_container(root)
                {
                    for arg in args {
                        if self.hir.type_kind(arg) == Some(HirTypeKind::Resource) {
                            self.resource_generic_argument_diagnostic(root, arg, span);
                        }
                    }
                }
                for arg in args {
                    self.check_resource_generic_calls_in_expr(&arg.value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_resource_generic_calls_in_expr(left);
                self.check_resource_generic_calls_in_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_resource_generic_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_generic_calls_in_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_resource_generic_calls_in_expr(base);
                self.check_resource_generic_calls_in_expr(index);
            }
            Expr::Closure { body, .. } => self.check_resource_generic_calls_in_block(body),
            Expr::Match { value, arms, .. } => {
                self.check_resource_generic_calls_in_expr(value);
                for arm in arms {
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.check_resource_generic_calls_in_expr(&entry.key);
                    self.check_resource_generic_calls_in_expr(&entry.value);
                }
            }
            Expr::ObjectLiteral { .. }
            | Expr::ArrayLiteral { .. }
            | Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::MultilineString(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    fn check_resource_pool_arg(&mut self, type_name: &str, span: &crate::diagnostic::Span) {
        match self.hir.type_kind(type_name) {
            Some(HirTypeKind::Resource) | None => {}
            Some(HirTypeKind::Class) | Some(HirTypeKind::Struct) | Some(HirTypeKind::Sum) => {
                self.invalid_resource_pool_type_diagnostic(
                    format!(
                        "ResourcePool can only hold resources, but `{type_name}` is not a resource."
                    ),
                    span.clone(),
                );
            }
        }
    }

    fn invalid_resource_pool_type_diagnostic(
        &mut self,
        summary: impl Into<String>,
        span: crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_RESOURCE_POOL_TYPE,
                summary,
                span,
                "invalid ResourcePool type",
            )
            .with_cause("`ResourcePool<T>` is the privileged container for long-lived resource values, so `T` must be a resource.")
            .with_fix(
                "use_resource_type",
                "Use a resource type argument or a non-resource container for ordinary values.",
                "manual",
            ),
        );
    }

    fn fd_surface_diagnostic(
        &mut self,
        span: crate::diagnostic::Span,
        summary: impl Into<String>,
        fix: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::FD_OUTSIDE_INTERNAL_BOUNDARY,
                summary,
                span,
                "Fd outside native/resource internals",
            )
            .with_cause(
                "`Fd` is a trusted native/resource-internal descriptor handle, not an ordinary RSScript value type.",
            )
            .with_fix("use_resource_wrapper", fix, "manual"),
        );
    }

    fn unknown_type_diagnostic(&mut self, ty: &TypeRef) {
        self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
    }

    fn unknown_type_name_diagnostic(&mut self, name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_TYPE,
                format!("unknown type `{name}`."),
                span.clone(),
                "unknown type",
            )
            .with_cause("RSScript type checking must resolve source-level types before Rust lowering.")
            .with_fix(
                "declare_or_import_type",
                format!(
                    "Declare `{}`, import an `.rssi` contract that declares it, or use a known core/runtime type.",
                    name
                ),
                "manual",
            ),
        );
    }

    fn unknown_protocol_diagnostic(&mut self, name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_PROTOCOL,
                format!("unknown protocol `{name}`."),
                span.clone(),
                "unknown protocol",
            )
            .with_cause(
                "Protocol bounds and implementations must name an explicit `protocol` declaration.",
            )
            .with_fix(
                "declare_protocol",
                format!("Declare `protocol {name} {{ ... }}` or use a declared protocol name."),
                "manual",
            ),
        );
    }

    fn protocol_impl_mismatch_diagnostic(
        &mut self,
        protocol: &str,
        type_name: &str,
        method: &str,
        span: &crate::diagnostic::Span,
        label: impl Into<String>,
        cause: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_INTERFACE_MISMATCH,
                format!("`{type_name}` does not satisfy protocol `{protocol}` method `{method}`."),
                span.clone(),
                label,
            )
            .with_cause(cause)
            .with_fix(
                "fix_protocol_impl_mapping",
                "Update the protocol impl mapping or concrete function signature to match the protocol contract exactly.",
                "manual",
            ),
        );
    }

    fn unknown_field_diagnostic(
        &mut self,
        field_name: &str,
        base_type: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_FIELD,
                format!("unknown field `{field_name}` on type `{base_type}`."),
                span.clone(),
                "unknown field",
            )
            .with_cause("RSScript field accesses must resolve before Rust lowering.")
            .with_fix(
                "use_declared_field",
                format!("Use a field declared on `{base_type}` or update the type declaration."),
                "manual",
            ),
        );
    }

    fn unknown_binding_diagnostic(&mut self, name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
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
            ),
        );
    }

    fn resource_generic_argument_diagnostic(
        &mut self,
        generic_name: &str,
        resource_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::RESOURCE_GENERIC_ARGUMENT,
                format!(
                    "generic type `{generic_name}` cannot be instantiated with resource `{resource_name}`."
                ),
                span.clone(),
                "resource generic argument",
            )
            .with_cause("Only explicit resource APIs such as `ResourcePool<T: Resource>` may hold resources.")
            .with_fix(
                "use_resource_api",
                "Use `with`, `ResourcePool<T: Resource>`, or a non-resource value type.",
                "manual",
            ),
        );
    }

    fn generic_resource_argument_diagnostic(
        &mut self,
        generic_name: &str,
        resource_name: &str,
        span: &crate::diagnostic::Span,
        cause: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::RESOURCE_GENERIC_ARGUMENT,
                format!(
                    "generic type `{generic_name}` cannot be used with resource `{resource_name}`."
                ),
                span.clone(),
                "resource generic misuse",
            )
            .with_cause(cause)
            .with_fix(
                "add_or_change_resource_bound",
                "Use explicit `T: Resource` only with approved resource APIs such as `ResourcePool<T>`.",
                "manual",
            ),
        );
    }

    fn noalloc_allocation_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
        cause: String,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NOALLOC_ALLOCATION,
                format!("`{function_name}` is declared noalloc but contains an allocation site."),
                span.clone(),
                "allocation in noalloc function",
            )
            .with_cause(cause)
            .with_fix(
                "remove_allocation_or_noalloc",
                "Remove the allocation site, or remove `noalloc` from the function effects.",
                "manual",
            ),
        );
    }

    fn allocating_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NOALLOC_CALL,
                format!(
                    "`{function_name}` is declared noalloc but calls possibly allocating function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly allocating call in noalloc function",
            )
            .with_cause(
                "A `noalloc` function may only call enum variants or functions also declared `effects(noalloc)`.",
            )
            .with_fix(
                "remove_noalloc_or_call_noalloc",
                "Remove `noalloc`, or call only APIs whose signatures are declared `effects(noalloc)`.",
                "manual",
            ),
        );
    }

    fn runtime_guarantee_call_diagnostic(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        match guarantee {
            RuntimeGuarantee::Noalloc => {
                self.allocating_call_diagnostic(function_name, callee, span)
            }
            RuntimeGuarantee::Pure => self.non_pure_call_diagnostic(function_name, callee, span),
            RuntimeGuarantee::NoBlock => self.blocking_call_diagnostic(function_name, callee, span),
            RuntimeGuarantee::NoPanic => self.panic_call_diagnostic(function_name, callee, span),
        }
    }

    fn non_pure_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!(
                    "`{function_name}` is declared pure but calls non-pure function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "non-pure call in pure function",
            )
            .with_cause(
                "A `pure` function may only call constructors, enum variants, or functions also declared `effects(pure)`.",
            )
            .with_fix(
                "remove_pure_or_call_pure",
                "Remove `pure`, or call only APIs whose signatures are declared `effects(pure)`.",
                "manual",
            ),
        );
    }

    fn pure_manage_diagnostic(&mut self, function_name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!("`{function_name}` is declared pure but uses `manage`."),
                span.clone(),
                "manage in pure function",
            )
            .with_cause(
                "`manage` consumes a local value and changes its ownership boundary; `pure` functions may observe inputs but must not consume local values.",
            )
            .with_fix(
                "remove_manage_or_pure",
                "Move the `manage` operation outside the pure function, or remove `pure`.",
                "manual",
            ),
        );
    }

    fn pure_with_resource_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!("`{function_name}` is declared pure but opens a resource scope."),
                span.clone(),
                "resource scope in pure function",
            )
            .with_cause(
                "`with` introduces deterministic resource lifetime behavior; `pure` functions may observe inputs but must not open resource scopes.",
            )
            .with_fix(
                "remove_with_or_pure",
                "Move resource handling outside the pure function, or remove `pure`.",
                "manual",
            ),
        );
    }

    fn pure_resource_return_diagnostic(
        &mut self,
        function_name: &str,
        span: crate::diagnostic::Span,
        resource_name: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!(
                    "`{function_name}` is declared pure but returns resource `{resource_name}`."
                ),
                span,
                "resource return in pure function",
            )
            .with_cause(
                "Returning a resource creates a lifetime boundary; `pure` functions must not open or return resources.",
            )
            .with_fix(
                "remove_resource_return_or_pure",
                "Return an ordinary value, or remove `pure` from the resource-producing function.",
                "manual",
            ),
        );
    }

    fn resource_return_type_name<'a>(&self, ty: &'a TypeRef) -> Option<&'a str> {
        let target = if matches!(ty.name.as_str(), "Result" | "Option") {
            ty.args.first().unwrap_or(ty)
        } else {
            ty
        };
        if self.hir.type_kind(&target.name) == Some(HirTypeKind::Resource) {
            Some(target.name.as_str())
        } else {
            None
        }
    }

    fn blocking_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NO_BLOCK_CALL,
                format!(
                    "`{function_name}` is declared no_block but calls possibly blocking function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly blocking call in no_block function",
            )
            .with_cause(
                "A `no_block` function may only call constructors, enum variants, or functions also declared `effects(no_block)`.",
            )
            .with_fix(
                "remove_no_block_or_call_no_block",
                "Remove `no_block`, or call only APIs whose signatures are declared `effects(no_block)`.",
                "manual",
            ),
        );
    }

    fn panic_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NO_PANIC_CALL,
                format!(
                    "`{function_name}` is declared no_panic but calls possibly panicking function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly panicking call in no_panic function",
            )
            .with_cause(
                "A `no_panic` function may only call constructors, enum variants, or functions also declared `effects(no_panic)`.",
            )
            .with_fix(
                "remove_no_panic_or_call_no_panic",
                "Remove `no_panic`, or call only APIs whose signatures are declared `effects(no_panic)`.",
                "manual",
            ),
        );
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
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => None,
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
            .or_else(|| arms.iter().find_map(|arm| block_first_await(&arm.body))),
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| expr_first_await(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| expr_first_await(&entry.value))
            }),
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
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
            other => stmt_first_await_ast(other),
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

#[derive(Debug, Clone, Copy)]
enum AssignBinding {
    MutLocal,
    ImmutableLocal,
    Param,
    /// A `mut` parameter: not reassignable itself, but its fields/elements may be
    /// updated in place (the mutation propagates to the caller, matching `&mut`).
    MutParam,
}

#[derive(Clone)]
struct AssignScopeEntry {
    binding: AssignBinding,
    type_name: Option<String>,
}

/// Validates controlled assignments: place/mutability rules plus a type check
/// (the value's type must match the place's type when both are known), so an
/// `Int = String` style error is reported in RSScript instead of leaking from
/// rustc. Binding kinds and their types are stored per lexical scope so an
/// inner shadow does not pollute the type of an outer same-named binding.
struct AssignChecker<'a> {
    hir: &'a Hir,
    scopes: Vec<HashMap<String, AssignScopeEntry>>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> AssignChecker<'a> {
    fn new(hir: &'a Hir) -> Self {
        Self {
            hir,
            scopes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn check_function(&mut self, function: &FunctionDecl) {
        self.scopes.clear();
        self.push_scope();
        for param in &function.params {
            let binding = if param.effect == Some(DataEffect::Mut) {
                AssignBinding::MutParam
            } else {
                AssignBinding::Param
            };
            self.insert(param.name.clone(), binding, Some(type_ref_name(&param.ty)));
        }
        self.block(&function.body);
        self.pop_scope();
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn insert(&mut self, name: String, binding: AssignBinding, type_name: Option<String>) {
        if name.is_empty() {
            return;
        }
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, AssignScopeEntry { binding, type_name });
        }
    }

    fn resolve(&self, name: &str) -> Option<AssignBinding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).map(|entry| entry.binding))
    }

    /// The type of the innermost binding named `name` (its own type, even when
    /// that is unknown), so a shadowing binding does not expose an outer type.
    fn resolve_type(&self, name: &str) -> Option<String> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .and_then(|entry| entry.type_name.clone())
    }

    /// A flattened view of every currently-visible binding's type, with inner
    /// scopes shadowing outer ones — including an untyped inner binding hiding
    /// an outer type — for inferring the type of a value expression.
    fn current_value_types(&self) -> HashMap<String, String> {
        let mut value_types = HashMap::new();
        for scope in &self.scopes {
            for (name, entry) in scope {
                match &entry.type_name {
                    Some(type_name) => {
                        value_types.insert(name.clone(), type_name.clone());
                    }
                    None => {
                        value_types.remove(name);
                    }
                }
            }
        }
        value_types
    }

    fn block(&mut self, block: &Block) {
        self.push_scope();
        for statement in &block.statements {
            self.stmt(statement);
        }
        self.pop_scope();
    }

    fn stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Assign(stmt) => {
                self.validate_assignment(stmt);
                self.expr(&stmt.value);
            }
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.expr(value);
                }
                let type_name = stmt
                    .type_annotation
                    .as_ref()
                    .map(type_ref_name)
                    .or_else(|| stmt.value.as_ref().and_then(|value| self.infer_type(value)));
                self.insert(
                    stmt.name.clone(),
                    if stmt.is_mut {
                        AssignBinding::MutLocal
                    } else {
                        AssignBinding::ImmutableLocal
                    },
                    type_name,
                );
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.expr(&stmt.resource);
                self.push_scope();
                self.insert(stmt.binding.clone(), AssignBinding::ImmutableLocal, None);
                self.block(&stmt.body);
                self.pop_scope();
            }
            Stmt::If(stmt) => {
                self.expr(&stmt.condition);
                self.block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.expr(condition);
                }
                self.block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.expr(&stmt.iterable);
                self.push_scope();
                self.insert(stmt.binding.clone(), AssignBinding::ImmutableLocal, None);
                self.block(&stmt.body);
                self.pop_scope();
            }
            Stmt::Match(stmt) => {
                self.expr(&stmt.value);
                self.match_arms(&stmt.arms);
            }
            Stmt::TaskGroup(stmt) => self.block(&stmt.body),
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    self.expr(&arm.operation);
                    self.block(&arm.body);
                }
            }
            Stmt::LetElse(stmt) => {
                self.expr(&stmt.value);
                self.block(&stmt.else_body);
                self.insert(
                    stmt.binding_name.clone(),
                    AssignBinding::ImmutableLocal,
                    None,
                );
            }
            Stmt::Expr(expr) => self.expr(expr),
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

    /// Walk into expressions that carry nested blocks (closures and match arms)
    /// so assignments inside them are validated against the right scope.
    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Closure { params, body, .. } => {
                self.push_scope();
                for param in params {
                    self.insert(param.clone(), AssignBinding::ImmutableLocal, None);
                }
                self.block(body);
                self.pop_scope();
            }
            Expr::Match { value, arms, .. } => {
                self.expr(value);
                self.match_arms(arms);
            }
            Expr::Binary { left, right, .. } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::Field { base, .. } => self.expr(base),
            Expr::Index { base, index, .. } => {
                self.expr(base);
                self.expr(index);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.expr(&arg.value);
                }
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => self.expr(value),
            Expr::MapLiteral { entries, .. } => {
                for entry in entries {
                    self.expr(&entry.key);
                    self.expr(&entry.value);
                }
            }
            Expr::ObjectLiteral { .. }
            | Expr::ArrayLiteral { .. }
            | Expr::Ident(..)
            | Expr::Number(..)
            | Expr::String(..)
            | Expr::MultilineString(..)
            | Expr::Unknown(_) => {}
        }
    }

    fn match_arms(&mut self, arms: &[crate::syntax::ast::MatchArm]) {
        for arm in arms {
            self.push_scope();
            for binding in arm.pattern.binding_names() {
                self.insert(binding.to_string(), AssignBinding::ImmutableLocal, None);
            }
            if let Some(guard) = &arm.guard {
                self.expr(guard);
            }
            self.block(&arm.body);
            self.pop_scope();
        }
    }

    fn infer_type(&self, value: &Expr) -> Option<String> {
        crate::hir::infer_hir_expr_type(self.hir, value, &self.current_value_types())
    }

    fn validate_assignment(&mut self, stmt: &AssignStmt) {
        let span = stmt.target.span().clone();
        match &stmt.target {
            Expr::Ident(name, _) => {
                let (label, cause) = match self.resolve(name) {
                    Some(AssignBinding::MutLocal) => {
                        self.check_assignment_type(name, &stmt.value, &span);
                        return;
                    }
                    Some(AssignBinding::ImmutableLocal) => (
                        format!("`{name}` is an immutable binding"),
                        format!("Declare `{name}` with `let mut` to allow reassignment."),
                    ),
                    Some(AssignBinding::Param | AssignBinding::MutParam) => (
                        format!("`{name}` is a parameter, not a reassignable local"),
                        "Parameters are not reassignable (even `mut` ones): a `mut` parameter's fields/elements may be updated, but the parameter binding itself can't be rebound. Bind a `let mut` local instead."
                            .to_string(),
                    ),
                    None => (
                        format!("`{name}` is not a binding in scope"),
                        "The left side of `=` must be a `let mut` local in scope.".to_string(),
                    ),
                };
                self.diagnostics
                    .push(invalid_assignment_diagnostic(span, label, cause));
            }
            Expr::Field { .. } | Expr::Index { .. } => {
                self.validate_compound_assignment(stmt, &span);
            }
            _ => self.diagnostics.push(invalid_assignment_diagnostic(
                span,
                "the left side of `=` is not a place".to_string(),
                "Assignment targets must be a place: a local, a field, or an index — not a call result or literal."
                    .to_string(),
            )),
        }
    }

    /// When both the place's type and the value's type are known, they must
    /// match. Unknown value types (such as the result of an unchecked binary
    /// expression) are left to the existing checks.
    fn check_assignment_type(&mut self, name: &str, value: &Expr, span: &crate::diagnostic::Span) {
        let Some(target_type) = self.resolve_type(name) else {
            return;
        };
        let Some(value_type) = self.infer_type(value) else {
            return;
        };
        if crate::checks::calls::unresolved_generic_type(&target_type)
            || crate::checks::calls::unresolved_generic_type(&value_type)
        {
            return;
        }
        if !crate::checks::calls::argument_type_matches(&target_type, &value_type) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::ASSIGNMENT_TYPE_MISMATCH,
                    format!("cannot assign `{value_type}` to `{name}` of type `{target_type}`."),
                    span.clone(),
                    "assignment type mismatch",
                )
                .with_cause(
                    "The assigned value's type must match the place's type before Rust lowering.",
                )
                .with_fix(
                    "match_assignment_type",
                    format!("Assign a `{target_type}` value to `{name}`."),
                    "manual",
                ),
            );
        }
    }

    fn validate_compound_assignment(&mut self, stmt: &AssignStmt, span: &crate::diagnostic::Span) {
        let Some(root) = assign_place_root(&stmt.target) else {
            self.diagnostics.push(invalid_assignment_diagnostic(
                span.clone(),
                "the left side of `=` is not rooted in a local place".to_string(),
                "Field and index assignment must start from a local binding, such as `user.name` or `items[i]`."
                    .to_string(),
            ));
            return;
        };
        match self.resolve(root) {
            Some(AssignBinding::MutLocal) => {}
            // A `mut` parameter's fields/elements may be updated in place (the
            // mutation propagates to the caller, like `&mut`).
            Some(AssignBinding::MutParam) => {}
            Some(AssignBinding::ImmutableLocal) => {
                self.diagnostics.push(invalid_assignment_diagnostic(
                    span.clone(),
                    format!("`{root}` is an immutable binding"),
                    format!("Declare `{root}` with `let mut` before assigning through one of its fields or indexes."),
                ));
                return;
            }
            Some(AssignBinding::Param) => {
                self.diagnostics.push(invalid_assignment_diagnostic(
                    span.clone(),
                    format!("`{root}` is a parameter, not a reassignable local"),
                    "Assign through a `mut` API parameter explicitly, or bind the value to a `let mut` local first."
                        .to_string(),
                ));
                return;
            }
            None => {
                self.diagnostics.push(invalid_assignment_diagnostic(
                    span.clone(),
                    format!("`{root}` is not a binding in scope"),
                    "The assignment target's root must be a `let mut` local in scope.".to_string(),
                ));
                return;
            }
        }

        if matches!(stmt.target, Expr::Index { .. }) {
            let Some(index_base) = assign_index_base(&stmt.target) else {
                return;
            };
            let Some(base_type) = self.infer_type(index_base) else {
                return;
            };
            if type_root_name(&base_type) != "List" {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::ASSIGNMENT_TARGET_DEFERRED,
                        "index assignment is only supported for List values.",
                        span.clone(),
                        format!("cannot assign through `{base_type}` index"),
                    )
                    .with_cause(
                        "`list[i] = value` has clear in-place list update semantics. Other indexed types still require explicit APIs such as `Map.insert`.",
                    )
                    .with_fix(
                        "use_explicit_update_api",
                        "Use the collection's explicit mutating API for this indexed assignment.",
                        "manual",
                    ),
                );
                return;
            }
        }

        self.check_assignment_place_type(&stmt.target, &stmt.value, span);
    }

    fn check_assignment_place_type(
        &mut self,
        target: &Expr,
        value: &Expr,
        span: &crate::diagnostic::Span,
    ) {
        let Some(target_type) = self.infer_type(target) else {
            return;
        };
        let Some(value_type) = self.infer_type(value) else {
            return;
        };
        if crate::checks::calls::unresolved_generic_type(&target_type)
            || crate::checks::calls::unresolved_generic_type(&value_type)
        {
            return;
        }
        if !crate::checks::calls::argument_type_matches(&target_type, &value_type) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::ASSIGNMENT_TYPE_MISMATCH,
                    format!("cannot assign `{value_type}` to `{target_type}` place."),
                    span.clone(),
                    "assignment type mismatch",
                )
                .with_cause(
                    "The assigned value's type must match the field or indexed element type before Rust lowering.",
                )
                .with_fix(
                    "match_assignment_type",
                    format!("Assign a `{target_type}` value to this place."),
                    "manual",
                ),
            );
        }
    }
}

fn invalid_assignment_diagnostic(
    span: crate::diagnostic::Span,
    label: String,
    cause: String,
) -> Diagnostic {
    Diagnostic::error(code::INVALID_ASSIGNMENT, "invalid assignment.", span, label)
        .with_cause(cause)
        .with_fix(
            "declare_let_mut",
            "Declare the target as a `let mut` local, or remove the assignment.",
            "manual",
        )
}

/// The root place identifier of an assignment target, following field/index
/// bases. Returns `None` when the target bottoms out at a non-place such as a
/// call result (`get_user().name = ...`).
fn assign_place_root(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => assign_place_root(base),
        _ => None,
    }
}

fn assign_index_base(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Index { base, .. } => Some(base),
        Expr::Field { base, .. } => assign_index_base(base),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeriveSupport {
    Yes,
    No,
    Unknown,
}

struct LocalTypeDerives {
    is_value: bool,
    derives: HashSet<String>,
}

/// The derives that impose a per-field requirement. `Debug`/`Clone` are
/// satisfied by every field type the checker accepts, and `Schema`/`ReviewSchema`
/// are review-only markers, so they impose nothing. `Ord` implies `Eq` in the
/// generated Rust, so a single `Ord` check covers both.
fn required_field_derives(derives: &[String]) -> Vec<&'static str> {
    let has = |name: &str| derives.iter().any(|derive| derive == name);
    let mut required = Vec::new();
    if has("Ord") {
        required.push("Ord");
    } else if has("Eq") {
        required.push("Eq");
    }
    if has("Hash") {
        required.push("Hash");
    }
    if has("JsonEncode") {
        required.push("JsonEncode");
    }
    if has("JsonDecode") {
        required.push("JsonDecode");
    }
    required
}

fn derive_requirement_cause(derive: &str) -> &'static str {
    match derive {
        "Eq" => {
            "`Eq` is total equality, so every field's type must be `Eq` (for example `Float` is not, and a struct or sum field must itself derive `Eq`)."
        }
        "Ord" => {
            "`Ord` is total ordering, so every field's type must be `Ord` (for example `Float` is not, and a struct or sum field must itself derive `Ord`)."
        }
        "Hash" => {
            "`Hash` requires hashable fields, so every field's type must be `Hash` (`Float`, `Map`, and `Set` are not, and a struct or sum field must itself derive `Hash`)."
        }
        "JsonEncode" => {
            "`JsonEncode` requires every struct or sum field's type to also derive `JsonEncode`."
        }
        "JsonDecode" => {
            "`JsonDecode` requires every struct or sum field's type to also derive `JsonDecode`."
        }
        _ => "Every field must support the derived trait.",
    }
}

fn combine_support(left: DeriveSupport, right: DeriveSupport) -> DeriveSupport {
    match (left, right) {
        (DeriveSupport::No, _) | (_, DeriveSupport::No) => DeriveSupport::No,
        (DeriveSupport::Unknown, _) | (_, DeriveSupport::Unknown) => DeriveSupport::Unknown,
        _ => DeriveSupport::Yes,
    }
}

/// Whether a field of type `ty` supports `derive`. Returns `Unknown` whenever
/// the verdict is not certain, so the checker only rejects known-bad cases and
/// never a program the backend would accept.
fn field_supports_derive(
    ty: &TypeRef,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    let root = type_root_name(&ty.name);
    if generic_params.contains(root) {
        // `#[derive(..)]` adds the matching `T: Trait` bound, so a generic
        // parameter field is valid at the definition site.
        return DeriveSupport::Unknown;
    }
    match derive {
        "Eq" | "Ord" | "Hash" => structural_support(ty, root, derive, local_types, generic_params),
        "JsonEncode" | "JsonDecode" => json_support(ty, root, derive, local_types, generic_params),
        _ => DeriveSupport::Unknown,
    }
}

fn structural_support(
    ty: &TypeRef,
    root: &str,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    match root {
        "Float" | "Float32" | "Float64" => DeriveSupport::No,
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" | "Bool" | "Byte" | "Char" | "Unit" | "String" => DeriveSupport::Yes,
        "List" | "Option" => match ty.args.first() {
            Some(arg) => field_supports_derive(arg, derive, local_types, generic_params),
            None => DeriveSupport::Unknown,
        },
        "Result" if !ty.args.is_empty() => ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
            combine_support(
                acc,
                field_supports_derive(arg, derive, local_types, generic_params),
            )
        }),
        // `HashSet<T>: Eq` requires `T: Eq + Hash`, so the element must be both.
        "Set" if derive == "Eq" => match ty.args.first() {
            Some(elem) => combine_support(
                field_supports_derive(elem, "Eq", local_types, generic_params),
                hash_key_support(elem, local_types, generic_params),
            ),
            None => DeriveSupport::Unknown,
        },
        // `HashMap<K, V>: Eq` requires `K: Eq + Hash` and `V: Eq`.
        "Map" if derive == "Eq" => match (ty.args.first(), ty.args.get(1)) {
            (Some(key), Some(value)) => combine_support(
                combine_support(
                    field_supports_derive(key, "Eq", local_types, generic_params),
                    hash_key_support(key, local_types, generic_params),
                ),
                field_supports_derive(value, "Eq", local_types, generic_params),
            ),
            _ => DeriveSupport::Unknown,
        },
        // `HashMap`/`HashSet` implement neither `Ord` nor `Hash`.
        "Map" | "Set" => DeriveSupport::No,
        _ => user_type_support(ty, root, derive, local_types, generic_params),
    }
}

/// Whether `ty` can serve as a hashable container key/element. A `Map` key or
/// `Set` element must be `Hash`, but the enclosing `Eq`/`JsonDecode` derive only
/// adds an `Eq`/`Deserialize` bound to a generic parameter — never `Hash` — and
/// RSScript has no `Hash` generic bound to express it. So this mirrors the
/// `Hash` structural rule but treats a generic parameter at *any* nesting depth
/// as `No`, catching nested positions such as `Map<List<T>, V>`.
fn hash_key_support(
    ty: &TypeRef,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    let root = type_root_name(&ty.name);
    if generic_params.contains(root) {
        return DeriveSupport::No;
    }
    match root {
        "Float" | "Float32" | "Float64" => DeriveSupport::No,
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" | "Bool" | "Byte" | "Char" | "Unit" | "String" => DeriveSupport::Yes,
        "List" | "Option" => match ty.args.first() {
            Some(arg) => hash_key_support(arg, local_types, generic_params),
            None => DeriveSupport::Unknown,
        },
        "Result" if !ty.args.is_empty() => ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
            combine_support(acc, hash_key_support(arg, local_types, generic_params))
        }),
        // `HashMap`/`HashSet` are not themselves `Hash`.
        "Map" | "Set" => DeriveSupport::No,
        _ => match local_types.get(root) {
            // A local type used as a key must derive `Hash`, and its `#[derive(Hash)]`
            // adds `Arg: Hash` for each generic argument.
            Some(local) if local.is_value => {
                if !local.derives.contains("Hash") {
                    return DeriveSupport::No;
                }
                ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
                    combine_support(acc, hash_key_support(arg, local_types, generic_params))
                })
            }
            _ => DeriveSupport::Unknown,
        },
    }
}

/// Whether a field supports `JsonEncode`/`JsonDecode`. Builtin scalars and
/// containers go through serde, but a `Deserialize` impl for `HashMap`/`HashSet`
/// additionally requires the key/element type to be `Eq + Hash`, so a `Float`
/// key is rejected for `JsonDecode`.
fn json_support(
    ty: &TypeRef,
    root: &str,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    let decode = derive == "JsonDecode";
    let recurse = |arg: &TypeRef| field_supports_derive(arg, derive, local_types, generic_params);
    // A `Deserialize` impl for `HashMap`/`HashSet` requires the key/element to be
    // `Eq + Hash`; `hash_key_support` rejects generic parameters in that position.
    let hashable_key = |arg: &TypeRef| {
        combine_support(
            field_supports_derive(arg, "Eq", local_types, generic_params),
            hash_key_support(arg, local_types, generic_params),
        )
    };
    match root {
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" | "Float" | "Float32" | "Float64" | "Bool" | "Byte" | "Char" | "Unit"
        | "String" => DeriveSupport::Yes,
        "List" | "Option" => match ty.args.first() {
            Some(arg) => recurse(arg),
            None => DeriveSupport::Unknown,
        },
        "Result" if !ty.args.is_empty() => ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
            combine_support(acc, recurse(arg))
        }),
        "Set" => match ty.args.first() {
            Some(arg) if decode => combine_support(recurse(arg), hashable_key(arg)),
            Some(arg) => recurse(arg),
            None => DeriveSupport::Unknown,
        },
        "Map" if ty.args.len() == 2 => {
            let key = &ty.args[0];
            let value = &ty.args[1];
            let elements = combine_support(recurse(key), recurse(value));
            if decode {
                combine_support(elements, hashable_key(key))
            } else {
                elements
            }
        }
        "Map" => DeriveSupport::Unknown,
        _ => user_type_support(ty, root, derive, local_types, generic_params),
    }
}

fn user_type_support(
    ty: &TypeRef,
    root: &str,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    match local_types.get(root) {
        Some(local) if local.is_value => {
            let derived = match derive {
                // An `Ord`-deriving value type also gets `Eq` in generated Rust.
                "Eq" => local.derives.contains("Eq") || local.derives.contains("Ord"),
                other => local.derives.contains(other),
            };
            if !derived {
                return DeriveSupport::No;
            }
            // The local type's `#[derive(D)]` adds `Arg: D` for each generic
            // argument, so `Key<Float>` deriving `Eq` still requires `Float: Eq`.
            ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
                combine_support(
                    acc,
                    field_supports_derive(arg, derive, local_types, generic_params),
                )
            })
        }
        _ => DeriveSupport::Unknown,
    }
}

fn supported_compiler_derive(name: &str) -> bool {
    // `Eq`/`Ord` are the canonical spellings; `PartialEq`/`PartialOrd` are not a
    // separate review-visible surface (Rust expands them inside `Eq`/`Ord`).
    matches!(
        name,
        "Debug"
            | "Clone"
            | "Eq"
            | "Ord"
            | "Hash"
            | "JsonEncode"
            | "JsonDecode"
            | "Schema"
            | "ReviewSchema"
    )
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
            .or_else(|| builtin_value_type_name(name)),
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
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn builtin_value_type_name(name: &str) -> Option<&'static str> {
    match name {
        "true" | "false" => Some("Bool"),
        "null" => Some("JsonLiteral"),
        "Unit" => Some("Unit"),
        _ => None,
    }
}



fn constructor_pattern_is_irrefutable(pattern: &MatchPattern) -> bool {
    match pattern {
        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => true,
        MatchPattern::Variant { binding: None, .. } => true,
        MatchPattern::Variant {
            binding: Some(binding),
            ..
        } => matches!(
            binding.as_ref(),
            MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
        ),
        MatchPattern::Struct { fields, .. } => fields.iter().all(|field| {
            field.pattern.is_none()
                || field.pattern.as_deref().is_some_and(|pattern| {
                    matches!(
                        pattern,
                        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                    )
                })
        }),
        MatchPattern::Literal { .. } => false,
    }
}

fn constrained_field_patterns(pattern: &MatchPattern) -> Vec<(String, &MatchPattern)> {
    match pattern {
        MatchPattern::Variant {
            binding: Some(binding),
            ..
        } if !matches!(
            binding.as_ref(),
            MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
        ) =>
        {
            vec![("value".to_string(), binding.as_ref())]
        }
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
        MatchPattern::Binding { .. } | MatchPattern::Literal { .. } | MatchPattern::Wildcard(_) => {
            Vec::new()
        }
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
            effect.as_str(),
            analyzer_expr_label(receiver)
        ),
    }
}

fn analyzer_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
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
