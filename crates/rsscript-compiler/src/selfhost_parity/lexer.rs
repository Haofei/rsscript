
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::diagnostic::{SELFHOST_CHECKER_TARGET_CODES, code};
use crate::interface_metadata::{
    collect_interface_metadata, format_selfhost_interface_metadata_rss,
};
use crate::interfaces::default_interfaces;
use crate::lexer::{TokenKind, lex};
use crate::vm_adapter::reg_vm_compile_sources;
use crate::syntax::ast::{Expr, Item, Stmt};
use crate::syntax::parse_source_raw;
use crate::{RegVmExecutable, Severity, analyze_source, review_package_dir};

/// One token in the canonical dump. `len` is a Unicode-scalar span length,
/// matching the Rust lexer spans and the RSS scanner's `String.chars` cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonTok {
    line: usize,
    col: usize,
    len: usize,
    kind: String,
    payload: String,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn selfhost_dir() -> PathBuf {
    workspace_root().join("selfhost")
}

fn env_tier_u8(var: &str, default: u8, allowed: &[u8]) -> u8 {
    let Some(value) = std::env::var(var).ok() else {
        return default;
    };
    let parsed = value
        .parse::<u8>()
        .unwrap_or_else(|_| panic!("{var} must be one of {allowed:?}, got {value:?}"));
    assert!(
        allowed.contains(&parsed),
        "{var} must be one of {allowed:?}, got {value:?}"
    );
    parsed
}

/// Comparison tier from `RSS_SELFHOST_TIER` (default 0). 0 = kind+payload,
/// 1 = +position, 2 = +Unicode-scalar span length.
fn tier() -> u8 {
    static T: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *T.get_or_init(|| env_tier_u8("RSS_SELFHOST_TIER", 0, &[0, 1, 2]))
}

fn env_flag_tier(var: &str) -> bool {
    match std::env::var(var).ok().as_deref() {
        Some("1") => true,
        Some("0") | None => false,
        Some(value) => panic!("{var} must be unset, 0, or 1, got {value:?}"),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn kind_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Ident(_) => "Ident",
        TokenKind::Number(_) => "Number",
        TokenKind::String(_) => "String",
        TokenKind::Char(_) => "Char",
        TokenKind::InterpolatedString(_) => "InterpolatedString",
        TokenKind::MultilineString(_) => "MultilineString",
        TokenKind::Keyword(_) => "Keyword",
        TokenKind::Symbol(_) => "Symbol",
        TokenKind::Unknown(_) => "Unknown",
        TokenKind::Eof => "Eof",
    }
}

fn payload(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(s)
        | TokenKind::Number(s)
        | TokenKind::String(s)
        | TokenKind::Char(s)
        | TokenKind::InterpolatedString(s)
        | TokenKind::MultilineString(s) => escape(s),
        TokenKind::Keyword(k) | TokenKind::Symbol(k) => escape(k),
        TokenKind::Unknown(c) => escape(&c.to_string()),
        TokenKind::Eof => String::new(),
    }
}

/// Truth: the real Rust lexer's canonical token stream.
fn oracle_dump(file: &str, source: &str) -> Vec<CanonTok> {
    lex(file, source)
        .iter()
        .map(|t| CanonTok {
            line: t.span.line,
            col: t.span.column,
            len: t.span.length,
            kind: kind_name(&t.kind).to_string(),
            payload: payload(&t.kind),
        })
        .collect()
}

/// Parse a `L:C:N\tKIND\tPAYLOAD` line into a token.
fn parse_line(line: &str) -> Option<CanonTok> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let pos = parts[0];
    let kind = parts[1].to_string();
    let payload = parts[2].to_string();
    if kind.is_empty() {
        return None;
    }
    let mut nums = pos.split(':');
    let line_no = nums.next()?.parse().ok()?;
    let col = nums.next()?.parse().ok()?;
    let len = nums.next()?.parse().ok()?;
    if nums.next().is_some() {
        return None;
    }
    Some(CanonTok {
        line: line_no,
        col,
        len,
        kind,
        payload,
    })
}

fn selfhost_import_to_tool(path: &[String], glob: bool) -> Option<String> {
    if path.len() >= 2 && path.first().is_some_and(|segment| segment == "selfhost") {
        let tool_path = if glob || path.len() == 2 {
            &path[1..]
        } else {
            &path[1..path.len() - 1]
        };
        if tool_path.len() == 1 && tool_path[0] == "interfaces" {
            return Some("generated/interface_metadata.rss".to_string());
        }
        Some(format!("{}.rss", tool_path.join("/")))
    } else {
        None
    }
}

fn generated_selfhost_source(tool: &str) -> Option<String> {
    if tool == "generated/interface_metadata.rss" {
        let interfaces = default_interfaces().collect::<Vec<_>>();
        let metadata = collect_interface_metadata(&interfaces);
        Some(format_selfhost_interface_metadata_rss(&metadata))
    } else {
        None
    }
}

fn selfhost_imports(file: &str, source: &str) -> Vec<String> {
    parse_source_raw(file, source)
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Use(decl) => selfhost_import_to_tool(&decl.path, decl.glob),
            _ => None,
        })
        .collect()
}

#[test]
fn selfhost_import_resolution_maps_symbols_to_owning_tool() {
    assert_eq!(
        selfhost_import_to_tool(&["selfhost".into(), "scan".into()], false).as_deref(),
        Some("scan.rss")
    );
    assert_eq!(
        selfhost_import_to_tool(&["selfhost".into(), "scan".into(), "Tok".into()], false)
            .as_deref(),
        Some("scan.rss")
    );
    assert_eq!(
        selfhost_import_to_tool(&["selfhost".into(), "scan".into()], true).as_deref(),
        Some("scan.rss")
    );
    assert_eq!(
        selfhost_import_to_tool(
            &["selfhost".into(), "interfaces".into(), "Lookup".into()],
            false
        )
        .as_deref(),
        Some("generated/interface_metadata.rss")
    );
}

/// Read a self-hosted tool and its declared `use selfhost.*` dependencies as
/// separate VM sources. `use` is not a filesystem loader in RSS itself; the
/// test-only harness resolves local selfhost modules before calling the normal
/// multi-source VM compiler.
fn tool_sources(tool: &str) -> Result<Vec<(String, String)>, String> {
    let dir = selfhost_dir();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([tool.to_string()]);
    let mut out = Vec::new();

    while let Some(current) = queue.pop_front() {
        if !seen.insert(current.clone()) {
            continue;
        }
        let source = if let Some(source) = generated_selfhost_source(&current) {
            source
        } else {
            let path = dir.join(&current);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        };
        for import in selfhost_imports(&format!("selfhost/{current}"), &source) {
            queue.push_back(import);
        }
        out.push((format!("selfhost/{current}"), source));
    }

    Ok(out)
}

thread_local! {
    /// `RegVmExecutable` owns `Rc` state and therefore cannot be shared across
    /// test threads. Reuse one compiled copy per worker thread instead: the
    /// self-hosted sources are immutable for the lifetime of this test binary,
    /// while many focused parity tests invoke the same tool.
    static SELFHOST_TOOL_CACHE: RefCell<BTreeMap<String, RegVmExecutable>> = const {
        RefCell::new(BTreeMap::new())
    };
}

fn compile_selfhost_tool(tool: &str, label: &str) -> Result<RegVmExecutable, String> {
    if let Some(executable) = SELFHOST_TOOL_CACHE.with(|cache| cache.borrow().get(tool).cloned()) {
        return Ok(executable);
    }
    let sources = tool_sources(tool)?;
    let source_refs = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let executable = reg_vm_compile_sources(&source_refs)
        .map_err(|e| format!("rss {label} failed to compile: {e:?}"))?;
    SELFHOST_TOOL_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(tool.to_string(), executable.clone());
    });
    Ok(executable)
}

fn bootstrap_runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("selfhost/runtime")
}

fn compile_bootstrap_c(c_file: &Path, binary: &Path) -> std::process::Output {
    let runtime = bootstrap_runtime_dir();
    Command::new("cc")
        .arg("-std=c11")
        .arg("-Wall")
        .arg("-Werror")
        .arg("-I")
        .arg(&runtime)
        .arg(c_file)
        .arg(runtime.join("rssrt.c"))
        .arg("-o")
        .arg(binary)
        .output()
        .expect("C compiler should be available in the Docker test image")
}

fn bootstrap_ir_expr(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) if matches!(name.as_str(), "Unit" | "true" | "false") => {
            format!("literal {name}")
        }
        Expr::Ident(name, _) => format!("name {name}"),
        Expr::Number(value, _) | Expr::String(value, _) | Expr::CharLiteral(value, _) => {
            format!("literal {value}")
        }
        Expr::ArrayLiteral { items, .. } => format!(
            "array[{}]",
            items
                .iter()
                .map(bootstrap_ir_expr)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Expr::Binary {
            op, left, right, ..
        } => format!(
            "bin {} ({}) ({})",
            match op {
                crate::syntax::ast::BinaryOp::Add => "+",
                crate::syntax::ast::BinaryOp::Subtract => "-",
                crate::syntax::ast::BinaryOp::Multiply => "*",
                crate::syntax::ast::BinaryOp::Divide => "/",
                crate::syntax::ast::BinaryOp::Modulo => "%",
                crate::syntax::ast::BinaryOp::BitAnd => "&",
                crate::syntax::ast::BinaryOp::BitOr => "|",
                crate::syntax::ast::BinaryOp::BitXor => "^",
                crate::syntax::ast::BinaryOp::ShiftLeft => "<<",
                crate::syntax::ast::BinaryOp::ShiftRight => ">>",
                crate::syntax::ast::BinaryOp::Equal => "==",
                crate::syntax::ast::BinaryOp::NotEqual => "!=",
                crate::syntax::ast::BinaryOp::Less => "<",
                crate::syntax::ast::BinaryOp::LessEqual => "<=",
                crate::syntax::ast::BinaryOp::Greater => ">",
                crate::syntax::ast::BinaryOp::GreaterEqual => ">=",
                crate::syntax::ast::BinaryOp::LogicalAnd => "&&",
                crate::syntax::ast::BinaryOp::LogicalOr => "||",
            },
            bootstrap_ir_expr(left),
            bootstrap_ir_expr(right)
        ),
        Expr::Field { base, name, .. } => format!("field {}.{name}", bootstrap_ir_expr(base)),
        Expr::Index { base, index, .. } => {
            format!(
                "index {}[{}]",
                bootstrap_ir_expr(base),
                bootstrap_ir_expr(index)
            )
        }
        Expr::Effect { effect, value, .. } => {
            format!("effect {} {}", effect.as_str(), bootstrap_ir_expr(value))
        }
        Expr::Manage { value, .. } => format!("manage {}", bootstrap_ir_expr(value)),
        Expr::Await { value, .. } => format!("await {}", bootstrap_ir_expr(value)),
        Expr::Try { value, .. } => format!("try {}", bootstrap_ir_expr(value)),
        Expr::Closure {
            params,
            captures,
            explicit,
            body,
            ..
        } if !*explicit && captures.is_empty() => {
            let body = if let [Stmt::Expr(value)] = body.statements.as_slice() {
                bootstrap_ir_expr(value)
            } else {
                match body
                    .statements
                    .iter()
                    .map(bootstrap_ir_inline_statement)
                    .collect::<Option<Vec<_>>>()
                {
                    Some(statements) if !statements.is_empty() => {
                        format!("block{{{}}}", statements.join(";"))
                    }
                    _ => "unsupported".to_string(),
                }
            };
            format!("closure({})=>{body}", params.join(","))
        }
        Expr::Call { callee, args, .. } => {
            let callee = match callee {
                crate::syntax::ast::Callee::Name(name) => name.clone(),
                crate::syntax::ast::Callee::Qualified { namespace, name } => {
                    format!("{namespace}.{name}")
                }
                crate::syntax::ast::Callee::ReceiverCall {
                    receiver, method, ..
                } => match receiver.as_ref() {
                    Expr::Ident(name, _) => format!("{name}.{method}"),
                    _ => return "unsupported".to_string(),
                },
            };
            let arguments = args
                .iter()
                .map(|argument| match &argument.name {
                    Some(name) => format!("{name}={}", bootstrap_ir_expr(&argument.value)),
                    None => bootstrap_ir_expr(&argument.value),
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("call {callee}({arguments})")
        }
        _ => "unsupported".to_string(),
    }
}

fn bootstrap_ir_type(ty: &crate::syntax::ast::TypeRef) -> String {
    let mut prefix = String::new();
    if ty.is_fresh {
        prefix.push_str("fresh ");
    }
    if ty.is_noescape {
        prefix.push_str("noescape ");
    }
    if ty.is_owned {
        prefix.push_str("owned ");
    }
    let arguments = ty.args.iter().map(bootstrap_ir_type).collect::<Vec<_>>();
    if arguments.is_empty() {
        format!("{prefix}{}", ty.name)
    } else {
        format!("{prefix}{}<{}>", ty.name, arguments.join(", "))
    }
}

fn bootstrap_ir_generics(generics: &[crate::syntax::ast::GenericParam]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    let values = generics
        .iter()
        .map(|generic| {
            let bound = match &generic.bound {
                None => return generic.name.clone(),
                Some(crate::syntax::ast::GenericBound::Managed) => "Managed".to_string(),
                Some(crate::syntax::ast::GenericBound::Struct) => "Struct".to_string(),
                Some(crate::syntax::ast::GenericBound::Resource) => "Resource".to_string(),
                Some(crate::syntax::ast::GenericBound::Protocol(name)) => name.clone(),
            };
            format!("{}:{bound}", generic.name)
        })
        .collect::<Vec<_>>();
    format!("<{}>", values.join(","))
}

fn bootstrap_ir_retains(lines: &mut Vec<String>, retained_params: &[String]) {
    for name in retained_params {
        lines.push(format!("  retains {name}"));
    }
}

fn bootstrap_ir_data(lines: &mut Vec<String>, decl: &crate::syntax::ast::TypeDecl) {
    let kind = match decl.kind {
        crate::syntax::ast::TypeKind::Class => "class",
        crate::syntax::ast::TypeKind::Struct => "struct",
        crate::syntax::ast::TypeKind::Resource => "resource",
    };
    let name = format!("{}{}", decl.name, bootstrap_ir_generics(&decl.type_params));
    lines.push(format!(
        "type {kind} {name} public={} opaque={}",
        decl.is_public, decl.is_opaque
    ));
    if !decl.derives.is_empty() {
        lines.push(format!("  derives {}", decl.derives.join(",")));
    }
    for field in &decl.fields {
        let mut line = format!("  field {}:{}", field.name, bootstrap_ir_type(&field.ty));
        if field.is_handle {
            line.push_str(" handle");
        }
        if field.is_weak {
            line.push_str(" weak");
        }
        lines.push(line);
    }
}

fn bootstrap_ir_sum(lines: &mut Vec<String>, decl: &crate::syntax::ast::SumTypeDecl) {
    let name = format!("{}{}", decl.name, bootstrap_ir_generics(&decl.type_params));
    lines.push(format!("sum {name} public={}", decl.is_public));
    if !decl.derives.is_empty() {
        lines.push(format!("  derives {}", decl.derives.join(",")));
    }
    for variant in &decl.variants {
        lines.push(format!("  variant {}", variant.name));
        for field in &variant.fields {
            let mut line = format!("    field {}:{}", field.name, bootstrap_ir_type(&field.ty));
            if field.is_handle {
                line.push_str(" handle");
            }
            if field.is_weak {
                line.push_str(" weak");
            }
            lines.push(line);
        }
    }
}

fn bootstrap_ir_alias(lines: &mut Vec<String>, decl: &crate::syntax::ast::TypeAliasDecl) {
    lines.push(format!(
        "alias {}{}={} public={}",
        decl.name,
        bootstrap_ir_generics(&decl.type_params),
        bootstrap_ir_type(&decl.target),
        decl.is_public
    ));
}

fn bootstrap_ir_const(lines: &mut Vec<String>, decl: &crate::syntax::ast::ConstDecl) {
    lines.push(format!(
        "const {}:{}={} public={}",
        decl.name,
        decl.type_annotation
            .as_ref()
            .map_or_else(String::new, bootstrap_ir_type),
        bootstrap_ir_expr(&decl.value),
        decl.is_public
    ));
}

fn bootstrap_ir_protocol(lines: &mut Vec<String>, decl: &crate::syntax::ast::ProtocolDecl) {
    lines.push(format!("protocol {}", decl.name));
}

fn bootstrap_ir_impl(lines: &mut Vec<String>, decl: &crate::syntax::ast::ProtocolImpl) {
    lines.push(format!("impl {} for {}", decl.protocol, decl.type_name));
    for mapping in &decl.mappings {
        lines.push(format!("  map {}={}", mapping.method, mapping.target));
    }
}

fn bootstrap_ir_module(lines: &mut Vec<String>, decl: &crate::syntax::ast::ModuleDecl) {
    lines.push(format!("module {}", decl.path.join(".")));
}

fn bootstrap_ir_use(lines: &mut Vec<String>, decl: &crate::syntax::ast::UseDecl) {
    let mut line = format!("use {}", decl.path.join("."));
    if decl.glob {
        line.push_str(".*");
    } else if let Some(alias) = &decl.alias {
        line.push_str(&format!(" as {alias}"));
    }
    lines.push(line);
}

fn bootstrap_ir_pattern(pattern: &crate::syntax::ast::MatchPattern) -> String {
    match pattern {
        crate::syntax::ast::MatchPattern::Binding { name, .. } => format!("name {name}"),
        crate::syntax::ast::MatchPattern::Literal { value, .. } => match value {
            crate::syntax::ast::MatchLiteral::Int(value)
            | crate::syntax::ast::MatchLiteral::String(value)
            | crate::syntax::ast::MatchLiteral::Char(value) => format!("literal {value}"),
            crate::syntax::ast::MatchLiteral::Bool(value) => format!("literal {value}"),
        },
        crate::syntax::ast::MatchPattern::Variant { name, bindings, .. } => {
            if bindings.is_empty() {
                format!("name {name}")
            } else {
                format!(
                    "call {name}({})",
                    bindings
                        .iter()
                        .map(bootstrap_ir_pattern)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
        _ => "unsupported".to_string(),
    }
}

fn bootstrap_ir_place(expression: &Expr) -> String {
    match expression {
        Expr::Ident(name, _) => name.clone(),
        _ => bootstrap_ir_expr(expression),
    }
}

fn bootstrap_ir_inline_statement(statement: &Stmt) -> Option<String> {
    match statement {
        Stmt::Let(binding) => {
            let mutability = if binding.is_mut { " mut" } else { "" };
            Some(format!(
                "let{mutability} {} = {}",
                binding.name,
                binding
                    .value
                    .as_ref()
                    .map_or_else(|| "unit".to_string(), bootstrap_ir_expr)
            ))
        }
        Stmt::Assign(assignment) => Some(format!(
            "assign {} = {}",
            bootstrap_ir_place(&assignment.target),
            bootstrap_ir_expr(&assignment.value)
        )),
        Stmt::Expr(expr) => Some(format!("expr {}", bootstrap_ir_expr(expr))),
        Stmt::Return(ret) => Some(format!(
            "return {}",
            ret.value
                .as_ref()
                .map_or_else(|| "unit".to_string(), bootstrap_ir_expr)
        )),
        _ => None,
    }
}

fn bootstrap_ir_statements(lines: &mut Vec<String>, statements: &[Stmt], depth: usize) {
    let prefix = "  ".repeat(depth);
    for statement in statements {
        match statement {
            Stmt::Let(binding) => {
                let mutability = if binding.is_mut { " mut" } else { "" };
                lines.push(format!(
                    "{prefix}let{mutability} {} = {}",
                    binding.name,
                    binding
                        .value
                        .as_ref()
                        .map_or_else(|| "unit".to_string(), bootstrap_ir_expr)
                ));
            }
            Stmt::Assign(assignment) => {
                lines.push(format!(
                    "{prefix}assign {} = {}",
                    bootstrap_ir_place(&assignment.target),
                    bootstrap_ir_expr(&assignment.value)
                ));
            }
            Stmt::Expr(expr) => lines.push(format!("{prefix}expr {}", bootstrap_ir_expr(expr))),
            Stmt::Return(ret) => lines.push(format!(
                "{prefix}return {}",
                ret.value
                    .as_ref()
                    .map_or_else(|| "unit".to_string(), bootstrap_ir_expr)
            )),
            Stmt::If(branch) => {
                lines.push(format!(
                    "{prefix}if {}",
                    bootstrap_ir_expr(&branch.condition)
                ));
                bootstrap_ir_statements(lines, &branch.then_body.statements, depth + 1);
                if let Some(else_body) = &branch.else_body {
                    lines.push(format!("{prefix}else"));
                    bootstrap_ir_statements(lines, &else_body.statements, depth + 1);
                }
                lines.push(format!("{prefix}end"));
            }
            Stmt::Loop(loop_stmt) => {
                match &loop_stmt.condition {
                    Some(condition) => {
                        lines.push(format!("{prefix}while {}", bootstrap_ir_expr(condition)));
                    }
                    None => lines.push(format!("{prefix}loop")),
                }
                bootstrap_ir_statements(lines, &loop_stmt.body.statements, depth + 1);
                lines.push(format!("{prefix}end"));
            }
            Stmt::For(for_stmt) => {
                lines.push(format!(
                    "{prefix}for {} in {}",
                    for_stmt.binding,
                    bootstrap_ir_expr(&for_stmt.iterable)
                ));
                bootstrap_ir_statements(lines, &for_stmt.body.statements, depth + 1);
                lines.push(format!("{prefix}end"));
            }
            Stmt::With(with_stmt) => {
                lines.push(format!(
                    "{prefix}with {} as {}",
                    bootstrap_ir_expr(&with_stmt.resource),
                    with_stmt.binding
                ));
                bootstrap_ir_statements(lines, &with_stmt.body.statements, depth + 1);
                lines.push(format!("{prefix}end"));
            }
            Stmt::Match(match_stmt) => {
                lines.push(format!(
                    "{prefix}match {}",
                    bootstrap_ir_expr(&match_stmt.value)
                ));
                for arm in &match_stmt.arms {
                    let mut arm_head =
                        format!("{prefix}  arm {}", bootstrap_ir_pattern(&arm.pattern));
                    if let Some(guard) = &arm.guard {
                        arm_head.push_str(&format!(" if {}", bootstrap_ir_expr(guard)));
                    }
                    lines.push(arm_head);
                    bootstrap_ir_statements(lines, &arm.body.statements, depth + 2);
                }
                lines.push(format!("{prefix}end"));
            }
            Stmt::Select(select_stmt) => {
                lines.push(format!("{prefix}select"));
                for arm in &select_stmt.arms {
                    lines.push(format!(
                        "{prefix}  arm {} = {}",
                        arm.binding,
                        bootstrap_ir_expr(&arm.operation)
                    ));
                    bootstrap_ir_statements(lines, &arm.body.statements, depth + 2);
                }
                lines.push(format!("{prefix}end"));
            }
            Stmt::TaskGroup(task_group) => {
                lines.push(format!("{prefix}task_group"));
                bootstrap_ir_statements(lines, &task_group.body.statements, depth + 1);
                lines.push(format!("{prefix}end"));
            }
            _ => lines.push(format!("{prefix}unsupported")),
        }
    }
}

fn rust_bootstrap_ir(source: &str) -> String {
    let program = parse_source_raw("bootstrap-ir.rss", source);
    let mut lines = vec!["rss-ir-v1".to_string()];
    for protocol in &program.protocols {
        bootstrap_ir_protocol(&mut lines, protocol);
    }
    for protocol_impl in &program.protocol_impls {
        bootstrap_ir_impl(&mut lines, protocol_impl);
    }
    for item in program.items {
        match item {
            Item::Module(decl) => bootstrap_ir_module(&mut lines, &decl),
            Item::Use(decl) => bootstrap_ir_use(&mut lines, &decl),
            Item::Type(decl) => bootstrap_ir_data(&mut lines, &decl),
            Item::SumType(decl) => bootstrap_ir_sum(&mut lines, &decl),
            Item::TypeAlias(decl) => bootstrap_ir_alias(&mut lines, &decl),
            Item::Const(decl) => bootstrap_ir_const(&mut lines, &decl),
            Item::Function(function) => {
                let parameters = function
                    .params
                    .iter()
                    .map(|parameter| {
                        let mut text = format!(
                            "{}:{} {}",
                            parameter.name,
                            parameter
                                .effective_effect()
                                .map_or("read", |effect| effect.as_str()),
                            bootstrap_ir_type(&parameter.ty)
                        );
                        if let Some(default) = &parameter.default {
                            text.push('=');
                            text.push_str(&bootstrap_ir_expr(default));
                        }
                        text
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                lines.push(format!(
                    "fn {}{}({parameters})->{}",
                    function.name,
                    bootstrap_ir_generics(&function.type_params),
                    function
                        .return_ty
                        .as_ref()
                        .map_or_else(String::new, bootstrap_ir_type)
                ));
                bootstrap_ir_retains(&mut lines, &function.retained_params);
                bootstrap_ir_statements(&mut lines, &function.body.statements, 1);
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

#[test]
fn selfhost_bootstrap_ir_matches_rust_oracle_for_straight_line_functions() {
    let source =
        "fn add(left: Int, right: Int) -> Int {\n    let sum = left + right\n    return sum\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(rust_bootstrap_ir(source), actual);
}

/// Stage-3 inner-loop gate: lowering must agree byte-for-byte over the explicit
/// lowering corpus. Unsupported syntax belongs in frontend/recovery tests, not
/// in this supported-subset contract.
fn canonical_ir_sample_files() -> Vec<PathBuf> {
    let dir = selfhost_dir().join("samples/ir");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rss"))
        .collect();
    files.sort();
    files
}

const CANONICAL_IR_SAMPLE_COUNT: usize = 5;

#[test]
fn canonical_ir_parity_samples() {
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss canonical IR lowerer should compile");
    let files = canonical_ir_sample_files();
    assert_eq!(
        files.len(),
        CANONICAL_IR_SAMPLE_COUNT,
        "canonical IR corpus must be explicit and complete; update the expected count with each reviewed sample change"
    );
    let mut mismatches = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let actual = executable
            .eval_main_with_args([source.clone()])
            .unwrap_or_else(|e| panic!("rss canonical IR failed for {}: {e:?}", file.display()))
            .stdout;
        let expected = rust_bootstrap_ir(&source);
        if actual != expected {
            mismatches.push(format!(
                "{}\n--- Rust IR ---\n{expected}\n--- RSS IR ---\n{actual}",
                file.display()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "canonical IR parity failed on {} curated samples:\n{}",
        mismatches.len(),
        mismatches.join("\n\n")
    );
}

#[test]
fn selfhost_c_emitter_compiles_and_runs_scalar_ir() {
    let executable = compile_selfhost_tool("backend/c_emit.rss", "bootstrap C emitter")
        .expect("rss C emitter should compile");
    let ir = "rss-ir-v1\nfn main()->Int\n  return literal 42\n";
    let c_source = executable
        .eval_main_with_args([ir.to_string()])
        .expect("rss C emitter should run")
        .stdout;
    assert!(c_source.contains("#include \"rssrt.h\""));
    assert!(c_source.contains("rssrt_print_int"));
    let rejected = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn main()->Int\n  return literal 42;system(\"false\")\n".to_string(),
        ])
        .expect("rss C emitter should reject malformed scalar IR without failing")
        .stdout;
    assert!(
        rejected.is_empty(),
        "C emitter must reject non-decimal IR literals"
    );
    let binary_ir = "rss-ir-v1\nfn main()->Int\n  return bin + (literal 2) (literal 40)\n";
    let binary_c_source = executable
        .eval_main_with_args([binary_ir.to_string()])
        .expect("rss C emitter should lower scalar binary IR")
        .stdout;
    assert!(
        binary_c_source.contains("(2+40)"),
        "C emitter must preserve the binary scalar expression"
    );
    let rejected_divide_by_zero = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn main()->Int\n  return bin / (literal 1) (literal 0)\n".to_string(),
        ])
        .expect("rss C emitter should reject invalid scalar binary IR without failing")
        .stdout;
    assert!(
        rejected_divide_by_zero.is_empty(),
        "C emitter must reject a statically invalid divide-by-zero expression"
    );
    let rejected_unknown_name = executable
        .eval_main_with_args(["rss-ir-v1\nfn main()->Int\n  return name missing\n".to_string()])
        .expect("rss C emitter should reject an undeclared scalar name without failing")
        .stdout;
    assert!(
        rejected_unknown_name.is_empty(),
        "C emitter must reject scalar IR that refers to an undeclared local"
    );
    let rejected_immutable_assignment = executable
        .eval_main_with_args(["rss-ir-v1\nfn main()->Int\n  let answer = literal 1\n  assign answer = literal 42\n  return name answer\n".to_string()])
        .expect("rss C emitter should reject immutable assignment without failing")
        .stdout;
    assert!(
        rejected_immutable_assignment.is_empty(),
        "C emitter must reject assignment to a non-mutable local"
    );
    let rejected_else_without_if = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn main()->Int\n  else\n  return literal 42\n".to_string()
        ])
        .expect("rss C emitter should reject a misplaced else without failing")
        .stdout;
    assert!(
        rejected_else_without_if.is_empty(),
        "C emitter must reject an else without a matching if"
    );
    let rejected_repeated_else = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn main()->Int\n  if literal 1\n  else\n  else\n  end\n  return literal 42\n"
                .to_string(),
        ])
        .expect("rss C emitter should reject a repeated else without failing")
        .stdout;
    assert!(
        rejected_repeated_else.is_empty(),
        "C emitter must reject a repeated else for one if"
    );
    let rejected_unknown_function = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn helper()->Int\n  return call missing()\nfn main()->Int\n  return call helper()\n"
                .to_string(),
        ])
        .expect("rss C emitter should reject an unknown helper without failing")
        .stdout;
    assert!(
        rejected_unknown_function.is_empty(),
        "C emitter must not emit a partial artifact for an unknown helper"
    );
    let rejected_unknown_pure_name = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn helper()->Int\n  return name missing\nfn main()->Int\n  return call helper()\n"
                .to_string(),
        ])
        .expect("rss C emitter should reject an undeclared pure name without failing")
        .stdout;
    assert!(
        rejected_unknown_pure_name.is_empty(),
        "C emitter must reject a bare name in a pure zero-argument helper"
    );
    let rejected_recursive_helper = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn loop()->Int\n  return call loop()\nfn main()->Int\n  return call loop()\n"
                .to_string(),
        ])
        .expect("rss C emitter should reject unsupported recursion without failing")
        .stdout;
    assert!(
        rejected_recursive_helper.is_empty(),
        "C emitter must reject recursive helpers outside the current ABI slice"
    );
    let rejected_indirect_recursion = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn first()->Int\n  return call second()\nfn second()->Int\n  return call first()\nfn main()->Int\n  return call first()\n"
                .to_string(),
        ])
        .expect("rss C emitter should reject indirect recursion without failing")
        .stdout;
    assert!(
        rejected_indirect_recursion.is_empty(),
        "C emitter must reject an indirect recursive helper cycle"
    );
    let rejected_forward_call = executable
        .eval_main_with_args([
            "rss-ir-v1\nfn first()->Int\n  return call second()\nfn second()->Int\n  return literal 42\nfn main()->Int\n  return call first()\n"
                .to_string(),
        ])
        .expect("rss C emitter should reject a forward helper call without failing")
        .stdout;
    assert!(
        rejected_forward_call.is_empty(),
        "C emitter must reject forward helper calls under its acyclic ABI rule"
    );

    let dir = selfhost_unique_temp_dir("rss-selfhost-c-emitter");
    std::fs::create_dir_all(&dir).expect("C emitter temp directory should be writable");
    let c_file = dir.join("main.c");
    let binary = dir.join("main");
    std::fs::write(&c_file, &c_source).expect("generated C should be writable");
    let compile = compile_bootstrap_c(&c_file, &binary);
    assert!(
        compile.status.success(),
        "generated C failed to compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&binary)
        .output()
        .expect("generated scalar C program should run");
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");

    std::fs::write(&c_file, binary_c_source).expect("generated binary C should be writable");
    let compile = compile_bootstrap_c(&c_file, &binary);
    assert!(
        compile.status.success(),
        "generated binary C failed to compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&binary)
        .output()
        .expect("generated scalar binary C program should run");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn selfhost_c_emitter_runs_canonical_ir_artifact() {
    let source = "fn main() -> Int {\n    let mut answer = 0\n    while answer < 42 {\n        if answer == 41 {\n            answer = 42\n        } else {\n            answer = answer + 1\n        }\n    }\n    return answer\n}\n";
    let lowerer = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss canonical IR lowerer should compile");
    let ir = lowerer
        .eval_main_with_args([source.to_string()])
        .expect("rss canonical IR lowerer should run")
        .stdout;
    let emitter = compile_selfhost_tool("backend/c_emit.rss", "bootstrap C emitter")
        .expect("rss C emitter should compile");
    let c_source = emitter
        .eval_main_with_args([ir.clone()])
        .expect("rss C emitter should consume canonical IR")
        .stdout;
    let dir = selfhost_unique_temp_dir("rss-selfhost-canonical-c");
    std::fs::create_dir_all(&dir).expect("C artifact directory should be writable");
    let c_file = dir.join("main.c");
    let binary = dir.join("main");
    std::fs::write(&c_file, &c_source).expect("generated C should be writable");
    let compile = compile_bootstrap_c(&c_file, &binary);
    assert!(
        compile.status.success(),
        "canonical C artifact failed to compile: {}\n--- generated C ---\n{}",
        String::from_utf8_lossy(&compile.stderr),
        c_source
    );
    let run = Command::new(&binary)
        .output()
        .expect("canonical C artifact should run");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn selfhost_c_emitter_runs_canonical_if_else_artifact() {
    let source = "fn main() -> Int {\n    let mut answer = 0\n    if true {\n        answer = 42\n    } else {\n        answer = 7\n    }\n    return answer\n}\n";
    let lowerer = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss canonical IR lowerer should compile");
    let ir = lowerer
        .eval_main_with_args([source.to_string()])
        .expect("rss canonical IR lowerer should run")
        .stdout;
    let emitter = compile_selfhost_tool("backend/c_emit.rss", "bootstrap C emitter")
        .expect("rss C emitter should compile");
    let c_source = emitter
        .eval_main_with_args([ir.clone()])
        .expect("rss C emitter should consume canonical if/else IR")
        .stdout;
    assert!(c_source.contains("if (1) {"));
    assert!(c_source.contains("} else {"));

    let dir = selfhost_unique_temp_dir("rss-selfhost-canonical-if-else-c");
    std::fs::create_dir_all(&dir).expect("C artifact directory should be writable");
    let c_file = dir.join("main.c");
    let binary = dir.join("main");
    std::fs::write(&c_file, &c_source).expect("generated C should be writable");
    let compile = compile_bootstrap_c(&c_file, &binary);
    assert!(
        compile.status.success(),
        "canonical if/else C artifact failed to compile: {}\n--- generated C ---\n{}",
        String::from_utf8_lossy(&compile.stderr),
        c_source
    );
    let run = Command::new(&binary)
        .output()
        .expect("canonical if/else C artifact should run");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn selfhost_c_emitter_runs_scalar_function_abi() {
    let source = "fn seed(value: Int) -> Int {\n    let next = value + 1\n    return next\n}\n\nfn relay() -> Int {\n    let base = 40\n    return seed(value: base + 1)\n}\n\nfn main() -> Int {\n    let result = relay()\n    return result\n}\n";
    let lowerer = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss canonical IR lowerer should compile");
    let ir = lowerer
        .eval_main_with_args([source.to_string()])
        .expect("rss canonical IR lowerer should run")
        .stdout;
    assert!(
        ir.contains("fn seed(value:read Int)->Int"),
        "canonical IR:\n{ir}"
    );
    let emitter = compile_selfhost_tool("backend/c_emit.rss", "bootstrap C emitter")
        .expect("rss C emitter should compile");
    let c_source = emitter
        .eval_main_with_args([ir.clone()])
        .expect("rss C emitter should consume pure function IR")
        .stdout;
    assert!(
        c_source.contains("static long long rss_fn_seed(long long value);"),
        "canonical IR:\n{ir}\n--- generated C ---\n{c_source}"
    );
    assert!(c_source.contains("long long next = (value+1);"));
    assert!(c_source.contains("return rss_fn_seed((base+1));"));
    assert!(c_source.contains("long long result = rss_fn_relay();"));

    let dir = selfhost_unique_temp_dir("rss-selfhost-canonical-function-abi-c");
    std::fs::create_dir_all(&dir).expect("C artifact directory should be writable");
    let c_file = dir.join("main.c");
    let binary = dir.join("main");
    std::fs::write(&c_file, &c_source).expect("generated C should be writable");
    let compile = compile_bootstrap_c(&c_file, &binary);
    assert!(
        compile.status.success(),
        "canonical function-ABI C artifact failed to compile: {}\n--- generated C ---\n{}",
        String::from_utf8_lossy(&compile.stderr),
        c_source
    );
    let run = Command::new(&binary)
        .output()
        .expect("canonical function-ABI C artifact should run");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

#[test]
fn selfhost_bootstrap_ir_lowers_named_calls() {
    let source = "fn run(value: Int) -> Int {\n    return helper(value: value)\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        "rss-ir-v1\nfn run(value:read Int)->Int\n  return call helper(value=name value)\n",
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_mutable_local_assignment() {
    let source = "fn increment(value: Int) -> Int {\n    let mut next = value\n    next = next + 1\n    return next\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn increment(value:read Int)->Int\n",
            "  let mut next = name value\n",
            "  assign next = bin + (name next) (literal 1)\n",
            "  return name next\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_if_else_blocks() {
    let source = "fn choose(value: Int) -> Int {\n    if value > 0 {\n        return value\n    } else {\n        return 0\n    }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn choose(value:read Int)->Int\n",
            "  if bin > (name value) (literal 0)\n",
            "    return name value\n",
            "  else\n",
            "    return literal 0\n",
            "  end\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_while_blocks() {
    let source = "fn countdown(value: Int) -> Int {\n    let mut next = value\n    while next > 0 {\n        next = next - 1\n    }\n    return next\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn countdown(value:read Int)->Int\n",
            "  let mut next = name value\n",
            "  while bin > (name next) (literal 0)\n",
            "    assign next = bin - (name next) (literal 1)\n",
            "  end\n",
            "  return name next\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_for_blocks() {
    let source = "fn sum(values: List<Int>) -> Int {\n    let mut total = 0\n    for item in values {\n        total = total + item\n    }\n    return total\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn sum(values:read List<Int>)->Int\n",
            "  let mut total = literal 0\n",
            "  for item in name values\n",
            "    assign total = bin + (name total) (name item)\n",
            "  end\n",
            "  return name total\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_with_blocks() {
    let source = "fn read_file(file: File) -> Unit {\n    with file as handle {\n        Output.write(message: read \"open\")\n    }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn read_file(file:read File)->Unit\n",
            "  with name file as handle\n",
            "    expr call Output.write(message=effect read literal open)\n",
            "  end\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_match_blocks() {
    let source = "fn classify(value: Int) -> Int {\n    match value {\n        0 => return 1\n        other => return other\n    }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn classify(value:read Int)->Int\n",
            "  match name value\n",
            "    arm literal 0\n",
            "      return literal 1\n",
            "    arm name other\n",
            "      return name other\n",
            "  end\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

/// Match lowering reads Pattern directly. This keeps the canonical IR oracle
/// independent of the removed Expr-backed MatchArm representation.
#[test]
fn selfhost_bootstrap_ir_lowers_positional_variant_patterns() {
    let source = "fn unwrap(value: Option<Int>) -> Int {\n    match value {\n        Some(item) => return item\n        None => return 0\n    }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn unwrap(value:read Option<Int>)->Int\n",
            "  match name value\n",
            "    arm call Some(name item)\n",
            "      return name item\n",
            "    arm name None\n",
            "      return literal 0\n",
            "  end\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_select_blocks() {
    let source = "fn pick(first: Chan, second: Chan) -> Unit {\n    select {\n        left = await first.receive() => { return Unit }\n        right = await second.receive() => return Unit\n    }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn pick(first:read Chan,second:read Chan)->Unit\n",
            "  select\n",
            "    arm left = await call first.receive()\n",
            "      return literal Unit\n",
            "    arm right = await call second.receive()\n",
            "      return literal Unit\n",
            "  end\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_task_group_blocks() {
    let source = "fn run() -> Unit {\n    task_group {\n        return Unit\n    }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn run()->Unit\n",
            "  task_group\n",
            "    return literal Unit\n",
            "  end\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_expression_statements() {
    let source =
        "fn trace(value: Int) -> Unit {\n    Output.write(message: \"value\")\n    return Unit\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn trace(value:read Int)->Unit\n",
            "  expr call Output.write(message=literal value)\n",
            "  return literal Unit\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_field_reads() {
    let source = "struct Config { value: Int }\nfn read_value(config: Config) -> Int {\n    return config.value\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "type struct Config public=false opaque=false\n",
            "  field value:Int\n",
            "fn read_value(config:read Config)->Int\n",
            "  return field name config.value\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_index_reads() {
    let source = "fn first(values: List<Int>) -> Int {\n    return values[0]\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn first(values:read List<Int>)->Int\n",
            "  return index name values[literal 0]\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_field_and_index_places() {
    let source = "struct Config { value: Int }\nfn update(config: Config, values: List<Int>, next: Int) -> Unit {\n    config.value = next\n    values[0] = next\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "type struct Config public=false opaque=false\n",
            "  field value:Int\n",
            "fn update(config:read Config,values:read List<Int>,next:read Int)->Unit\n",
            "  assign field name config.value = name next\n",
            "  assign index name values[literal 0] = name next\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_value_effects_and_await() {
    let source = "fn consume(value: read Int) -> Unit { return Unit }\nfn forward(value: read Int) -> Unit {\n    consume(value: read value)\n    return Unit\n}\nasync fn wait_for(task: read Task<Int>) -> Int {\n    return await task\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn consume(value:read Int)->Unit\n",
            "  return literal Unit\n",
            "fn forward(value:read Int)->Unit\n",
            "  expr call consume(value=effect read name value)\n",
            "  return literal Unit\n",
            "fn wait_for(task:read Task<Int>)->Int\n",
            "  return await name task\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_manage() {
    let source = "fn share(image: Image) -> Image {\n    return manage image\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn share(image:read Image)->Image\n",
            "  return manage name image\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_array_literals() {
    let source = "fn values() -> List<Int> {\n    return [1, 2 + 3]\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn values()->List<Int>\n",
            "  return array[literal 1,bin + (literal 2) (literal 3)]\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_postfix_try() {
    let source = "async fn wait_for() -> Result<Unit, Error> {\n    await Timer.sleep(ms: 1)?\n    return Ok(Unit)\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn wait_for()->Result<Unit, Error>\n",
            "  expr try await call Timer.sleep(ms=literal 1)\n",
            "  return call Ok(literal Unit)\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_generic_function_signatures() {
    let source =
        "fn identity<T: Managed, U>(value: read T, fallback: read U) -> T {\n    return value\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn identity<T:Managed,U>(value:read T,fallback:read U)->T\n",
            "  return name value\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_data_declarations() {
    let source = "pub struct Boxed<T: Managed> {\n    value: handle T\n}\n\nstruct Tagged derives(Eq, Hash) {\n    id: Int\n    label: String\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "type struct Boxed<T:Managed> public=true opaque=false\n",
            "  field value:T handle\n",
            "type struct Tagged public=false opaque=false\n",
            "  derives Eq,Hash\n",
            "  field id:Int\n",
            "  field label:String\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_sum_alias_and_const_declarations() {
    let source = "pub sum Result<T, E> derives(Eq) {\n    Ok(value: T)\n    Err(error: E)\n}\n\npub type Name<T> = List<T>\npub const LIMIT: Int = 3\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "sum Result<T,E> public=true\n",
            "  derives Eq\n",
            "  variant Ok\n",
            "    field value:T\n",
            "  variant Err\n",
            "    field error:E\n",
            "alias Name<T>=List<T> public=true\n",
            "const LIMIT:Int=literal 3 public=true\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_protocols_and_implementations() {
    let source = "protocol Render {\n    fn draw(self: read Self) -> Unit\n}\n\nimpl Render for Canvas {\n    draw = Canvas.draw\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "protocol Render\n",
            "impl Render for Canvas\n",
            "  map draw=Canvas.draw\n",
            "fn Render.draw<Self:Managed>(self:read Self)->Unit\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_module_and_use_declarations() {
    let source = "module package.review\nuse package.contract.Contract as PackageContract\nuse package.util.*\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "module package.review\n",
            "use package.contract.Contract as PackageContract\n",
            "use package.util.*\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_pipe_closures() {
    let source = "fn map_one(values: List<Int>) -> List<Int> {\n    return List.map(items: values, fn: |value| value + 1)\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn map_one(values:read List<Int>)->List<Int>\n",
            "  return call List.map(items=name values,fn=closure(value)=>bin + (name value) (literal 1))\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_braced_pipe_closures() {
    let source = "fn map_one(values: List<Int>) -> List<Int> {\n    return List.map(items: values, fn: |value| { value + 1 })\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn map_one(values:read List<Int>)->List<Int>\n",
            "  return call List.map(items=name values,fn=closure(value)=>bin + (name value) (literal 1))\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_braced_pipe_closure_statement_blocks() {
    let source = "fn map_two(values: List<Int>) -> List<Int> {\n    return List.map(items: values, fn: |value| {\n        let next = value + 1\n        next\n    })\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn map_two(values:read List<Int>)->List<Int>\n",
            "  return call List.map(items=name values,fn=closure(value)=>block{let next = bin + (name value) (literal 1);expr name next})\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_lowers_retention_contracts() {
    let source = "fn publish(value: read String) -> Unit\n    retains(value)\n{\n    return Unit\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        concat!(
            "rss-ir-v1\n",
            "fn publish(value:read String)->Unit\n",
            "  retains value\n",
            "  return literal Unit\n",
        ),
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

#[test]
fn selfhost_bootstrap_ir_marks_unmodelled_nodes_explicitly() {
    let source = "fn run(value: Int) -> Int {\n    return fn() { value }\n}\n";
    let executable = compile_selfhost_tool("ir/canonical.rss", "canonical bootstrap IR")
        .expect("rss bootstrap IR should compile");
    let actual = executable
        .eval_main_with_args([source.to_string()])
        .expect("rss bootstrap IR should run")
        .stdout;
    assert_eq!(
        "rss-ir-v1\nfn run(value:read Int)->Int\n  return unsupported\n",
        actual
    );
    assert_eq!(rust_bootstrap_ir(source), actual);
}

/// Compile `selfhost/lexer.rss` with the shared scanner once for reuse.
fn compile_lexer() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("lexer.rss", "lexer")
}

/// Run a precompiled rss lexer over `source` and parse its dump.
fn rss_dump_with(exe: &RegVmExecutable, source: &str) -> Result<Vec<CanonTok>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss lexer failed to run: {e:?}"))?;
    // Fail on any malformed non-empty dump line rather than silently dropping it
    // (a stray debug line or a garbled token would otherwise vanish and let a
    // broken lexer pass parity by emitting fewer/no tokens).
    output
        .stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            parse_line(l).ok_or_else(|| format!("rss lexer emitted a malformed dump line: {l:?}"))
        })
        .collect()
}

/// Convenience: compile + run once (used by the single-file smoke test).
fn rss_dump(source: &str) -> Result<Vec<CanonTok>, String> {
    rss_dump_with(&compile_lexer()?, source)
}

#[test]
fn lexer_output_parser_rejects_malformed_lines() {
    assert!(parse_line("1:2:3\tIdent\tname").is_some());
    assert!(parse_line("1:2\tIdent\tname").is_none());
    assert!(parse_line("1:2:x\tIdent\tname").is_none());
    assert!(parse_line("1:2:3:4\tIdent\tname").is_none());
    assert!(parse_line("1:2:3\tIdent\tname\textra").is_none());
    assert!(parse_line("1:2:3\t\tname").is_none());
}

/// Compare two token streams at the active tier; `Ok(())` or a diff message.
fn compare(oracle: &[CanonTok], actual: &[CanonTok], tier: u8) -> Result<(), String> {
    let field = |t: &CanonTok| match tier {
        0 => format!("{}\t{}", t.kind, t.payload),
        1 => format!("{}:{}\t{}\t{}", t.line, t.col, t.kind, t.payload),
        _ => format!("{}:{}:{}\t{}\t{}", t.line, t.col, t.len, t.kind, t.payload),
    };
    let n = oracle.len().max(actual.len());
    for i in 0..n {
        let o = oracle.get(i).map(field);
        let a = actual.get(i).map(field);
        if o != a {
            return Err(format!(
                "token #{i} diverges (tier {tier}):\n  oracle: {o:?}\n  rss:    {a:?}\n  \
                 (oracle {} tokens, rss {} tokens)",
                oracle.len(),
                actual.len()
            ));
        }
    }
    Ok(())
}

fn is_corpus_excluded_dir(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == "target"
        || name == ".git"
        || path
            .components()
            .any(|component| component.as_os_str() == ".claude")
}

#[test]
fn corpus_excludes_local_agent_worktrees() {
    assert!(is_corpus_excluded_dir(std::path::Path::new(
        "/repo/.claude/worktrees/review"
    )));
    assert!(!is_corpus_excluded_dir(std::path::Path::new(
        "/repo/tests/fixtures"
    )));
}

#[test]
fn corpus_manifest_matches_discovery() {
    let root = workspace_root();
    let manifest_path = selfhost_dir().join("corpus.txt");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", manifest_path.display()));
    let mut expected = manifest
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    expected.sort();
    let mut actual = collect_rss_files(&root)
        .expect("corpus discovery should succeed")
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    actual.sort();
    assert_eq!(
        actual, expected,
        "selfhost/corpus.txt is stale; update it when adding/removing corpus .rss files"
    );
}

/// Recursively collect `*.rss` files under `root`, skipping build output and
/// local agent worktrees. The self-host corpus must be hermetic to this checkout;
/// mirrored worktrees under `.claude/` duplicate fixtures and make gate counts
/// depend on local tooling state.
fn collect_rss_files(root: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read entry in {}: {e}", dir.display()))?;
            let path = entry.path();
            if path.is_dir() {
                if is_corpus_excluded_dir(&path) {
                    continue;
                }
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rss") {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Phase-0 proof: the rss lexer matches the Rust lexer on a tiny sample.
#[test]
fn lexer_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = oracle_dump("samples/tiny.rss", &source);
    let actual = rss_dump(&source).expect("rss lexer should run");
    compare(&oracle, &actual, tier()).unwrap_or_else(|msg| panic!("{msg}"));
}

/// Phase-1 gate (ignored by default; run with `-- --ignored`): the rss lexer
/// matches the Rust lexer over the whole `.rss` corpus. Prints a summary of
/// divergences (run-failures vs token mismatches) before asserting all pass.
#[test]
#[ignore]
fn lexer_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    let tier = tier();
    let exe = compile_lexer().expect("rss lexer should compile");
    let mut run_failures: Vec<String> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut ok = 0usize;
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
        let oracle = oracle_dump(&rel, &source);
        match rss_dump_with(&exe, &source) {
            Err(e) => run_failures.push(format!("{rel}: {e}")),
            Ok(actual) => match compare(&oracle, &actual, tier) {
                Ok(()) => ok += 1,
                Err(msg) => mismatches.push(format!("{rel}: {msg}")),
            },
        }
    }
    let total = files.len();
    eprintln!(
        "\n=== lexer_parity_corpus (tier {tier}) ===\n  files: {total}\n  ok: {ok}\n  \
         run-failures: {}\n  token-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(15) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(15) {
        eprintln!("[mismatch] {line}");
    }
    assert!(
        run_failures.is_empty() && mismatches.is_empty(),
        "lexer parity failed: {} run-failures, {} mismatches (of {total})",
        run_failures.len(),
        mismatches.len()
    );
}

/// Phase-4 perf probe (ignored; run with `--release -- --ignored --nocapture`):
/// how much slower is the self-hosted rss lexer (on the reg-VM) than the native
/// Rust `lex()` over the whole corpus? This is the "is the self-hosted tool
/// slow?" macro-benchmark — a real workload, not a microkernel. Feeds the parked
/// VM value-representation / intrinsic-dispatch perf work.
#[test]
#[ignore]
fn lexer_perf_corpus() {
    use std::time::Instant;
    let root = workspace_root();
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    let exe = compile_lexer().expect("rss lexer should compile");
    let mut rust_ns: u128 = 0;
    let mut rss_ns: u128 = 0;
    let mut bytes: usize = 0;
    let mut n_ok = 0usize;
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
        bytes += source.len();
        let t0 = Instant::now();
        let _ = lex(&rel, &source);
        rust_ns += t0.elapsed().as_nanos();
        let t1 = Instant::now();
        if exe.eval_main_with_args([source.clone()]).is_ok() {
            n_ok += 1;
        }
        rss_ns += t1.elapsed().as_nanos();
    }
    let rust_ms = rust_ns as f64 / 1e6;
    let rss_ms = rss_ns as f64 / 1e6;
    eprintln!(
        "\n=== lexer_perf_corpus ===\n  files: {} (ran {n_ok})\n  bytes: {bytes}\n  \
         Rust lex():   {rust_ms:.1} ms  ({:.1} MB/s)\n  rss lexer/VM: {rss_ms:.1} ms  \
         ({:.1} MB/s)\n  slowdown (rss/Rust): {:.1}x\n",
        files.len(),
        bytes as f64 / 1e6 / (rust_ms / 1e3),
        bytes as f64 / 1e6 / (rss_ms / 1e3),
        rss_ms / rust_ms,
    );
}
