// ---------------------------------------------------------------------------
// AST-dump parity — format contract + Rust oracle (step 1 of frontend object
// parity). The rss parser will one day emit the canonical AST dump defined in
// `docs/self-hosting.md`; this oracle emits it from the surface-preserving
// tree (`crate::syntax::parse_source_raw`, NOT the desugared `parse_source`).
// Byte-identical dumps = AST parity. This ships BEFORE `parser.rss` builds an
// AST, exactly as the token dump contract + oracle preceded the rss lexer.
//
// The serializer is TOTAL over the AST (every Item/Stmt/Expr/Pattern variant is
// rendered) so a future producer cannot pass by silently dropping a node. Tier 0:
// structure + payload, spans omitted (span parity is the final phase).
// ---------------------------------------------------------------------------

use crate::syntax::ast;

fn push_line(out: &mut String, depth: usize, content: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(content);
    out.push('\n');
}

/// AST-dump span tier from `RSS_SELFHOST_AST_TIER` (default 0). 0 = structure +
/// payload only (spans omitted); 1 = append ` @line:col` to every spanned node
/// head line; 2 = append ` @line:col:len`. Mirrors the lexer/parser tier ladders.
/// Cached once per process (a corpus run is single-tier).
fn ast_tier() -> u8 {
    static T: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *T.get_or_init(|| env_tier_u8("RSS_SELFHOST_AST_TIER", 0, &[0, 1, 2]))
}

/// Span suffix for a node head line at the active AST tier (empty at tier 0).
fn sp(span: &crate::diagnostic::Span) -> String {
    match ast_tier() {
        1 => format!(" @{}:{}", span.line, span.column),
        2 => format!(" @{}:{}:{}", span.line, span.column, span.length),
        _ => String::new(),
    }
}

/// Push a spanned node head line: `content` plus the tier's span suffix.
fn push_node(out: &mut String, depth: usize, content: &str, span: &crate::diagnostic::Span) {
    let mut c = String::from(content);
    c.push_str(&sp(span));
    push_line(out, depth, &c);
}

fn type_kind_str(k: ast::TypeKind) -> &'static str {
    match k {
        ast::TypeKind::Class => "class",
        ast::TypeKind::Struct => "struct",
        ast::TypeKind::Resource => "resource",
    }
}

fn let_kind_str(k: ast::LetKind) -> &'static str {
    match k {
        ast::LetKind::Managed => "managed",
        ast::LetKind::Local => "local",
    }
}

fn binop_name(op: ast::BinaryOp) -> &'static str {
    match op {
        ast::BinaryOp::Add => "add",
        ast::BinaryOp::Subtract => "subtract",
        ast::BinaryOp::Multiply => "multiply",
        ast::BinaryOp::Divide => "divide",
        ast::BinaryOp::Modulo => "modulo",
        ast::BinaryOp::BitAnd => "bit-and",
        ast::BinaryOp::BitOr => "bit-or",
        ast::BinaryOp::BitXor => "bit-xor",
        ast::BinaryOp::ShiftLeft => "shift-left",
        ast::BinaryOp::ShiftRight => "shift-right",
        ast::BinaryOp::Equal => "equal",
        ast::BinaryOp::NotEqual => "not-equal",
        ast::BinaryOp::Less => "less",
        ast::BinaryOp::LessEqual => "less-equal",
        ast::BinaryOp::Greater => "greater",
        ast::BinaryOp::GreaterEqual => "greater-equal",
        ast::BinaryOp::LogicalAnd => "logical-and",
        ast::BinaryOp::LogicalOr => "logical-or",
    }
}

/// Truth: the canonical AST dump of the surface-preserving parse tree.
fn ast_oracle_dump(file: &str, source: &str) -> String {
    let program = crate::syntax::parse_source_raw(file, source);
    let mut out = String::new();
    push_line(&mut out, 0, "program");
    for item in &program.items {
        dump_item(&mut out, 1, item);
    }
    for p in &program.protocols {
        push_line(&mut out, 1, &format!("protocol {}", p.name));
    }
    for pi in &program.protocol_impls {
        push_line(
            &mut out,
            1,
            &format!(
                "protocol-impl protocol={} type={}",
                pi.protocol, pi.type_name
            ),
        );
        for m in &pi.mappings {
            push_line(
                &mut out,
                2,
                &format!("mapping method={} target={}", m.method, m.target),
            );
        }
    }
    for _ in &program.unknown_top_level_spans {
        push_line(&mut out, 1, "unknown-top-level");
    }
    for _ in &program.malformed_declaration_spans {
        push_line(&mut out, 1, "malformed-declaration");
    }
    out
}

fn dump_item(out: &mut String, depth: usize, item: &ast::Item) {
    match item {
        ast::Item::Module(m) => {
            push_node(
                out,
                depth,
                &format!("module path={}", m.path.join(".")),
                &m.span,
            );
        }
        ast::Item::Use(u) => {
            let mut line = format!("use path={} glob={}", u.path.join("."), u.glob);
            if let Some(a) = &u.alias {
                line.push_str(&format!(" alias={a}"));
            }
            push_node(out, depth, &line, &u.span);
        }
        ast::Item::Type(t) => {
            push_node(
                out,
                depth,
                &format!(
                    "type kind={} name={} public={} opaque={}",
                    type_kind_str(t.kind),
                    t.name,
                    t.is_public,
                    t.is_opaque
                ),
                &t.span,
            );
            for g in &t.type_params {
                dump_generic(out, depth + 1, g);
            }
            for d in &t.derives {
                push_line(out, depth + 1, &format!("derive {d}"));
            }
            for f in &t.fields {
                dump_field(out, depth + 1, f);
            }
            for _ in &t.malformed_generic_param_spans {
                push_line(out, depth + 1, "malformed-generic");
            }
            for _ in &t.malformed_field_spans {
                push_line(out, depth + 1, "malformed-field");
            }
            if let Some(b) = &t.drop_body {
                push_line(out, depth + 1, "drop");
                dump_block(out, depth + 2, b);
            }
        }
        ast::Item::SumType(s) => {
            push_node(
                out,
                depth,
                &format!("sum name={} public={}", s.name, s.is_public),
                &s.span,
            );
            for g in &s.type_params {
                dump_generic(out, depth + 1, g);
            }
            for d in &s.derives {
                push_line(out, depth + 1, &format!("derive {d}"));
            }
            for v in &s.variants {
                push_node(out, depth + 1, &format!("variant name={}", v.name), &v.span);
                for f in &v.fields {
                    dump_field(out, depth + 2, f);
                }
            }
        }
        ast::Item::TypeAlias(a) => {
            push_node(
                out,
                depth,
                &format!("type-alias name={} public={}", a.name, a.is_public),
                &a.span,
            );
            for g in &a.type_params {
                dump_generic(out, depth + 1, g);
            }
            dump_type_ref(out, depth + 1, &a.target, "target", None);
        }
        ast::Item::Const(c) => {
            push_node(
                out,
                depth,
                &format!("const name={} public={}", c.name, c.is_public),
                &c.span,
            );
            if let Some(t) = &c.type_annotation {
                dump_type_ref(out, depth + 1, t, "type", None);
            }
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &c.value);
        }
        ast::Item::Function(f) => dump_function(out, depth, f),
    }
}

fn dump_function(out: &mut String, depth: usize, f: &ast::FunctionDecl) {
    let mut line = format!(
        "fn name={} public={} async={} has-body={}",
        f.name, f.is_public, f.is_async, f.has_body
    );
    if f.default_impl_marker {
        line.push_str(" default-impl=true");
    }
    if f.returns_fresh {
        line.push_str(" returns-fresh=true");
    }
    push_node(out, depth, &line, &f.span);
    if let Some(r) = &f.deprecated_reason {
        push_line(out, depth + 1, &format!("deprecated {}", escape(r)));
    }
    if let Some(l) = &f.lower_name {
        push_line(out, depth + 1, &format!("lower-name {l}"));
    }
    for g in &f.type_params {
        dump_generic(out, depth + 1, g);
    }
    for p in &f.params {
        dump_param(out, depth + 1, p);
    }
    if let Some(r) = &f.return_ty {
        dump_type_ref(out, depth + 1, r, "return-type", None);
    }
    for param in &f.retained_params {
        push_line(out, depth + 1, &format!("retains {param}"));
    }
    for _ in &f.malformed_generic_param_spans {
        push_line(out, depth + 1, "malformed-generic");
    }
    for _ in &f.malformed_param_spans {
        push_line(out, depth + 1, "malformed-param");
    }
    push_line(out, depth + 1, "body");
    dump_block(out, depth + 2, &f.body);
}

fn dump_generic(out: &mut String, depth: usize, g: &ast::GenericParam) {
    push_node(out, depth, &format!("generic name={}", g.name), &g.span);
    if let Some(b) = &g.bound {
        let s = match b {
            ast::GenericBound::Managed => "bound managed".to_string(),
            ast::GenericBound::Struct => "bound struct".to_string(),
            ast::GenericBound::Resource => "bound resource".to_string(),
            ast::GenericBound::Protocol(p) => format!("bound protocol={p}"),
        };
        push_line(out, depth + 1, &s);
    }
}

fn dump_field(out: &mut String, depth: usize, f: &ast::FieldDecl) {
    push_node(
        out,
        depth,
        &format!(
            "field name={} handle={} weak={}",
            f.name, f.is_handle, f.is_weak
        ),
        &f.span,
    );
    dump_type_ref(out, depth + 1, &f.ty, "type", None);
    if let Some(d) = &f.default {
        push_line(out, depth + 1, "default");
        dump_expr(out, depth + 2, d);
    }
}

fn dump_param(out: &mut String, depth: usize, p: &ast::Param) {
    let mut line = format!("param name={}", p.name);
    if let Some(e) = p.effect {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    push_node(out, depth, &line, &p.span);
    dump_type_ref(out, depth + 1, &p.ty, "type", None);
    if let Some(d) = &p.default {
        push_line(out, depth + 1, "default");
        dump_expr(out, depth + 2, d);
    }
}

fn dump_type_ref(
    out: &mut String,
    depth: usize,
    tr: &ast::TypeRef,
    tag: &str,
    eff: Option<ast::DataEffect>,
) {
    let mut line = String::from(tag);
    if let Some(e) = eff {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    line.push_str(&format!(
        " name={} fresh={} noescape={} owned={}",
        tr.name, tr.is_fresh, tr.is_noescape, tr.is_owned
    ));
    push_node(out, depth, &line, &tr.span);
    for a in &tr.args {
        dump_type_ref(out, depth + 1, a, "arg", None);
    }
    for (i, p) in tr.fn_params.iter().enumerate() {
        let e = tr.fn_param_effects.get(i).copied().flatten();
        dump_type_ref(out, depth + 1, p, "fn-param", e);
    }
    if let Some(r) = &tr.fn_return {
        dump_type_ref(out, depth + 1, r, "fn-return", None);
    }
}

fn dump_block(out: &mut String, depth: usize, b: &ast::Block) {
    push_line(out, depth, "block");
    for s in &b.statements {
        dump_stmt(out, depth + 1, s);
    }
}

#[allow(clippy::too_many_arguments)]
fn dump_match(
    out: &mut String,
    depth: usize,
    value: &ast::Expr,
    eff: Option<ast::DataEffect>,
    arms: &[ast::MatchArm],
    malformed_arms: usize,
    tag: &str,
    head_span: &crate::diagnostic::Span,
) {
    let mut line = String::from(tag);
    if let Some(e) = eff {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    push_node(out, depth, &line, head_span);
    push_line(out, depth + 1, "value");
    dump_expr(out, depth + 2, value);
    for arm in arms {
        push_node(out, depth + 1, "arm", &arm.span);
        push_line(out, depth + 2, "pattern");
        dump_pattern(out, depth + 3, &arm.pattern);
        if let Some(g) = &arm.guard {
            push_line(out, depth + 2, "guard");
            dump_expr(out, depth + 3, g);
        }
        dump_block(out, depth + 2, &arm.body);
    }
    for _ in 0..malformed_arms {
        push_line(out, depth + 1, "malformed-arm");
    }
}

fn dump_stmt(out: &mut String, depth: usize, s: &ast::Stmt) {
    match s {
        ast::Stmt::Let(l) => {
            let mut line = format!(
                "let kind={} name={} mut={} async={}",
                let_kind_str(l.kind),
                l.name,
                l.is_mut,
                l.is_async
            );
            if l.malformed {
                line.push_str(" malformed=true");
            }
            if let Some(names) = &l.destructure {
                line.push_str(&format!(" destructure={}", names.join(",")));
            }
            push_node(out, depth, &line, &l.span);
            if let Some(t) = &l.type_annotation {
                dump_type_ref(out, depth + 1, t, "type", None);
            }
            if let Some(v) = &l.value {
                push_line(out, depth + 1, "value");
                dump_expr(out, depth + 2, v);
            }
        }
        ast::Stmt::Return(r) => {
            push_node(out, depth, "return", &r.span);
            if let Some(v) = &r.value {
                push_line(out, depth + 1, "value");
                dump_expr(out, depth + 2, v);
            }
        }
        ast::Stmt::With(w) => {
            push_node(out, depth, &format!("with binding={}", w.binding), &w.span);
            push_line(out, depth + 1, "resource");
            dump_expr(out, depth + 2, &w.resource);
            dump_block(out, depth + 1, &w.body);
        }
        ast::Stmt::MalformedWith(s) => push_node(out, depth, "malformed-with", s),
        ast::Stmt::If(i) => {
            push_node(out, depth, "if", &i.span);
            push_line(out, depth + 1, "cond");
            dump_expr(out, depth + 2, &i.condition);
            push_line(out, depth + 1, "then");
            dump_block(out, depth + 2, &i.then_body);
            if let Some(e) = &i.else_body {
                push_line(out, depth + 1, "else");
                dump_block(out, depth + 2, e);
            }
        }
        ast::Stmt::MalformedIf(s) => push_node(out, depth, "malformed-if", s),
        ast::Stmt::Loop(l) => {
            push_node(out, depth, "loop", &l.span);
            if let Some(c) = &l.condition {
                push_line(out, depth + 1, "cond");
                dump_expr(out, depth + 2, c);
            }
            dump_block(out, depth + 1, &l.body);
        }
        ast::Stmt::MalformedLoop(s) => push_node(out, depth, "malformed-loop", s),
        ast::Stmt::For(f) => {
            push_node(
                out,
                depth,
                &format!("for binding={} async={}", f.binding, f.is_async),
                &f.span,
            );
            push_line(out, depth + 1, "iter");
            dump_expr(out, depth + 2, &f.iterable);
            dump_block(out, depth + 1, &f.body);
        }
        ast::Stmt::MalformedFor(s) => push_node(out, depth, "malformed-for", s),
        ast::Stmt::Match(m) => dump_match(
            out,
            depth,
            &m.value,
            m.scrutinee_effect,
            &m.arms,
            m.malformed_arm_spans.len(),
            "match",
            &m.span,
        ),
        ast::Stmt::MalformedMatch(s) => push_node(out, depth, "malformed-match", s),
        ast::Stmt::TaskGroup(t) => {
            push_node(out, depth, "task-group", &t.span);
            dump_block(out, depth + 1, &t.body);
        }
        ast::Stmt::Select(s) => {
            push_node(out, depth, "select", &s.span);
            for arm in &s.arms {
                push_line(
                    out,
                    depth + 1,
                    &format!("select-arm binding={}", arm.binding),
                );
                push_line(out, depth + 2, "operation");
                dump_expr(out, depth + 3, &arm.operation);
                dump_block(out, depth + 2, &arm.body);
            }
        }
        ast::Stmt::Break(s) => push_node(out, depth, "break", s),
        ast::Stmt::Continue(s) => push_node(out, depth, "continue", s),
        ast::Stmt::LetElse(l) => {
            push_node(
                out,
                depth,
                &format!("let-else binding={}", l.binding_name),
                &l.span,
            );
            push_line(out, depth + 1, "pattern");
            dump_pattern(out, depth + 2, &l.pattern);
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &l.value);
            push_line(out, depth + 1, "else");
            dump_block(out, depth + 2, &l.else_body);
        }
        ast::Stmt::Assign(a) => {
            push_node(out, depth, "assign", &a.span);
            push_line(out, depth + 1, "target");
            dump_expr(out, depth + 2, &a.target);
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &a.value);
        }
        ast::Stmt::Expr(e) => {
            push_node(out, depth, "expr-stmt", e.span());
            dump_expr(out, depth + 1, e);
        }
        ast::Stmt::Unknown(s) => push_node(out, depth, "unknown-stmt", s),
    }
}

fn dump_pattern(out: &mut String, depth: usize, p: &ast::MatchPattern) {
    match p {
        ast::MatchPattern::Binding { name, .. } => {
            push_line(out, depth, &format!("pat-binding name={name}"));
        }
        ast::MatchPattern::Variant { name, bindings, .. } => {
            push_line(out, depth, &format!("pat-variant name={name}"));
            for b in bindings {
                dump_pattern(out, depth + 1, b);
            }
        }
        ast::MatchPattern::Struct {
            name,
            fields,
            has_rest,
            ..
        } => {
            push_line(
                out,
                depth,
                &format!("pat-struct name={name} rest={has_rest}"),
            );
            for f in fields {
                let mut line = format!("pat-field name={} ignored={}", f.name, f.ignored);
                if let Some(b) = &f.binding {
                    line.push_str(&format!(" binding={b}"));
                }
                if let Some(e) = f.effect {
                    line.push_str(&format!(" effect={}", e.as_str()));
                }
                push_line(out, depth + 1, &line);
                if let Some(sub) = &f.pattern {
                    dump_pattern(out, depth + 2, sub);
                }
            }
        }
        ast::MatchPattern::Literal { value, .. } => {
            let (kind, payload) = match value {
                ast::MatchLiteral::Int(s) => ("int", escape(s)),
                ast::MatchLiteral::String(s) => ("string", escape(s)),
                ast::MatchLiteral::Char(s) => ("char", escape(s)),
                ast::MatchLiteral::Bool(b) => ("bool", b.to_string()),
            };
            push_line(out, depth, &format!("pat-literal kind={kind} {payload}"));
        }
        ast::MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } => {
            let rest_s = match rest {
                None => "none".to_string(),
                Some(None) => "ignore".to_string(),
                Some(Some(n)) => n.clone(),
            };
            push_line(out, depth, &format!("pat-list rest={rest_s}"));
            if !prefix.is_empty() {
                push_line(out, depth + 1, "list-prefix");
                for pp in prefix {
                    dump_pattern(out, depth + 2, pp);
                }
            }
            if !suffix.is_empty() {
                push_line(out, depth + 1, "list-suffix");
                for pp in suffix {
                    dump_pattern(out, depth + 2, pp);
                }
            }
        }
        ast::MatchPattern::Wildcard(_) => push_line(out, depth, "pat-wildcard"),
    }
}

fn dump_callee(out: &mut String, depth: usize, c: &ast::Callee) {
    match c {
        ast::Callee::Name(n) => push_line(out, depth, &format!("callee-name name={n}")),
        ast::Callee::Qualified { namespace, name } => push_line(
            out,
            depth,
            &format!("callee-qualified namespace={namespace} name={name}"),
        ),
        ast::Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => {
            let mut line = format!("callee-receiver method={method}");
            if let Some(e) = effect {
                line.push_str(&format!(" effect={}", e.as_str()));
            }
            push_line(out, depth, &line);
            dump_expr(out, depth + 1, receiver);
        }
    }
}

fn dump_expr(out: &mut String, depth: usize, e: &ast::Expr) {
    let es = e.span();
    match e {
        ast::Expr::Ident(s, _) => push_node(out, depth, &format!("ident {}", escape(s)), es),
        ast::Expr::Number(s, _) => push_node(out, depth, &format!("number {}", escape(s)), es),
        ast::Expr::String(s, _) => push_node(out, depth, &format!("string {}", escape(s)), es),
        ast::Expr::CharLiteral(s, _) => push_node(out, depth, &format!("char {}", escape(s)), es),
        ast::Expr::MultilineString(s, _) => {
            push_node(out, depth, &format!("multiline {}", escape(s)), es)
        }
        ast::Expr::ObjectLiteral { fields, .. } => {
            push_node(out, depth, "object", es);
            for f in fields {
                push_line(out, depth + 1, &format!("object-field name={}", f.name));
                dump_expr(out, depth + 2, &f.value);
            }
        }
        ast::Expr::MapLiteral { entries, .. } => {
            push_node(out, depth, "map", es);
            for en in entries {
                push_line(out, depth + 1, "map-entry");
                push_line(out, depth + 2, "key");
                dump_expr(out, depth + 3, &en.key);
                push_line(out, depth + 2, "value");
                dump_expr(out, depth + 3, &en.value);
            }
        }
        ast::Expr::ArrayLiteral { items, .. } => {
            push_node(out, depth, "array", es);
            for it in items {
                dump_expr(out, depth + 1, it);
            }
        }
        ast::Expr::Binary {
            op, left, right, ..
        } => {
            push_node(out, depth, &format!("binary op={}", binop_name(*op)), es);
            dump_expr(out, depth + 1, left);
            dump_expr(out, depth + 1, right);
        }
        ast::Expr::Field { base, name, .. } => {
            push_node(out, depth, &format!("field-access name={name}"), es);
            dump_expr(out, depth + 1, base);
        }
        ast::Expr::Index { base, index, .. } => {
            push_node(out, depth, "index", es);
            dump_expr(out, depth + 1, base);
            dump_expr(out, depth + 1, index);
        }
        ast::Expr::Call { callee, args, .. } => {
            push_node(out, depth, "call", es);
            dump_callee(out, depth + 1, callee);
            for a in args {
                let mut line = String::from("arg");
                if let Some(n) = &a.name {
                    line.push_str(&format!(" name={n}"));
                }
                if a.malformed {
                    line.push_str(" malformed=true");
                }
                push_line(out, depth + 1, &line);
                dump_expr(out, depth + 2, &a.value);
            }
        }
        ast::Expr::Effect { effect, value, .. } => {
            push_node(out, depth, &format!("effect kind={}", effect.as_str()), es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Manage { value, .. } => {
            push_node(out, depth, "manage", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Spawn { value, .. } => {
            push_node(out, depth, "spawn", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Await { value, .. } => {
            push_node(out, depth, "await", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Try { value, .. } => {
            push_node(out, depth, "try", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Closure {
            params,
            captures,
            explicit,
            body,
            ..
        } => {
            push_node(out, depth, &format!("closure explicit={explicit}"), es);
            for p in params {
                push_line(out, depth + 1, &format!("closure-param {p}"));
            }
            for c in captures {
                push_line(
                    out,
                    depth + 1,
                    &format!("capture effect={} name={}", c.effect.as_str(), c.name),
                );
            }
            push_line(out, depth + 1, "body");
            dump_block(out, depth + 2, body);
        }
        ast::Expr::Match {
            value,
            scrutinee_effect,
            arms,
            from_if_expression,
            malformed_arm_spans,
            span,
        } => dump_match(
            out,
            depth,
            value,
            *scrutinee_effect,
            arms,
            malformed_arm_spans.len(),
            if *from_if_expression {
                "if-expr"
            } else {
                "match-expr"
            },
            span,
        ),
        ast::Expr::Unknown(_) => push_node(out, depth, "unknown-expr", es),
    }
}

/// Phase-5 proof (non-ignored): the AST oracle is deterministic and total —
/// dumping the tiny sample twice is identical, non-empty, and the serializer
/// panics on no node (totality is exercised more broadly by the corpus test).
#[test]
fn ast_oracle_dump_is_deterministic_smoke() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let a = ast_oracle_dump("samples/tiny.rss", &source);
    let b = ast_oracle_dump("samples/tiny.rss", &source);
    assert_eq!(a, b, "AST oracle dump must be deterministic");
    assert!(!a.is_empty(), "AST oracle dump must be non-empty");
    assert!(a.starts_with("program\n"), "dump must start with `program`");
}

/// Phase-5 golden (non-ignored): pins the exact AST dump of the tiny sample so
/// the format contract in `docs/self-hosting.md` is locked BEFORE `parser.rss`
/// targets it. When the rss parser is built, its dump must equal this byte for
/// byte at tier 0.
#[test]
fn ast_oracle_dump_tiny_sample_golden() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let dump = ast_oracle_dump("samples/tiny.rss", &source);
    let expected = "\
program
  fn name=add public=false async=false has-body=true
    param name=x
      type name=Int fresh=false noescape=false owned=false
    return-type name=Int fresh=false noescape=false owned=false
    body
      block
        return
          value
            binary op=add
              ident x
              number 1
";
    assert_eq!(
        dump, expected,
        "AST dump golden mismatch\n--- actual ---\n{dump}"
    );
}

#[test]
fn astdump_parity_if_expression() {
    let source = "fn choose(flag: Bool) -> Int {\n    return if flag {\n        7\n    } else {\n        11\n    }\n}\n";
    let oracle = ast_oracle_dump("if-expression.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

#[test]
fn astdump_parity_long_left_associative_binary_chain() {
    let source = "fn any_flag(a: Bool, b: Bool, c: Bool, d: Bool, e: Bool, f: Bool, g: Bool, h: Bool, i: Bool, j: Bool, k: Bool, l: Bool, m: Bool, n: Bool, o: Bool, p: Bool) -> Bool {\n    return a || b || c || d || e || f || g || h || i || j || k || l || m || n || o || p\n}\n";
    let oracle = ast_oracle_dump("long-binary-chain.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// The first direct shared-body renderer consumes return expressions from the
/// materialized Program. Keep precedence in this focused gate: a flat,
/// token-order-only binary split would render `(a + b) * c` here.
#[test]
fn astdump_shared_body_precedence_parity() {
    let source = "fn compute(a: Int, b: Int) -> Int { return a + b * 2 - 1 }\n";
    let oracle = ast_oracle_dump("shared-body-precedence.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// Guard the direct body slice's binding and qualified-call representation.
/// `local` and an absent/malformed initializer are distinct AST facts, while
/// named arguments must keep their value nesting and dump order.
#[test]
fn astdump_shared_body_let_and_call_parity() {
    let source = "fn size(xs: List<Int>) -> Int {\n    local count = List.len(list: xs)\n    return count\n}\n";
    let oracle = ast_oracle_dump("shared-body-let-call.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

#[test]
fn astdump_shared_body_receiver_call_parity() {
    let source = "fn display(report: Report) -> String { return report.format() }\n";
    let oracle = ast_oracle_dump("shared-body-receiver-call.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// Keep the shared renderer's non-control expression and mutation slice exact.
/// These nodes are already materialized by `Program`; this gate prevents them
/// from silently returning to the legacy token renderer.

#[test]
fn astdump_shared_body_control_flow_parity() {
    let source = r#"fn choose(flag: Bool, values: List<Int>) -> Int {
    if flag {
        return 1
    } else {
        return 2
    }
    while flag {
        return 3
    }
    for value in values {
        return value
    }
    task_group {
        return 4
    }
    loop {
        return 5
    }
}
"#;
    let oracle = ast_oracle_dump("shared-body-control-flow.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

#[test]
fn astdump_shared_body_with_parity() {
    let source = r#"fn read(path: Path) -> Int {
    with File.open_read(path) as file {
        return 1
    }
}
"#;
    let oracle = ast_oracle_dump("shared-body-with.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

#[test]
fn astdump_shared_body_select_parity() {
    let source = r#"fn wait(first: Task<Int>, second: Task<Int>) -> Int {
    select {
        left = await first => { return left }
        right = await second => { return right }
    }
}
"#;
    let oracle = ast_oracle_dump("shared-body-select.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// Common variant and binding patterns are represented directly by the shared
/// expression arena. Keep this direct match slice byte-exact; richer pattern
/// forms deliberately retain the legacy renderer until Pattern is shared too.
#[test]
fn astdump_shared_body_match_simple_pattern_parity() {
    let source = r#"fn unwrap(value: Option<Int>) -> Int {
    match value {
        Some(item) if item > 0 => { return item }
        None => { return 0 }
    }
}
"#;
    let oracle = ast_oracle_dump("shared-body-match-simple-pattern.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// Struct patterns now use the shared Pattern arena, including shorthand and
/// effect-qualified bindings, ignored fields, nested constructors, and `..`.
#[test]
fn astdump_shared_body_match_struct_pattern_parity() {
    let source = r#"fn radius(shape: Shape) -> Int {
    match shape {
        Circle { radius: read value, center: Point { x, y: _ }, .. } => { return value }
        _ => { return 0 }
    }
}
"#;
    let oracle = ast_oracle_dump("shared-body-match-struct-pattern.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// Tuple and list patterns retain the canonical synthetic tuple fields and the
/// list rest split, including a suffix after the captured rest.
#[test]
fn astdump_shared_body_match_tuple_list_pattern_parity() {
    let source = r#"fn choose(pair: Pair, values: List<Int>) -> Int {
    match pair {
        (left, _) => { return left }
    }
    match values {
        [first, ..middle, last] => { return first }
        [] => { return 0 }
    }
}
"#;
    let oracle = ast_oracle_dump("shared-body-match-tuple-list-pattern.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

#[test]
fn astdump_shared_body_pipe_closure_parity() {
    let source = r#"fn apply(value: Int) -> Int {
    let increment = |item| item + 1
    let doubled = |item| {
        let next = item * 2
        return next
    }
    return increment(value: doubled(value: value))
}
"#;
    let oracle = ast_oracle_dump("shared-body-pipe-closure.rss", source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, source).expect("rss astdump should run");
    assert_eq!(actual, oracle);
}

/// Phase-5 totality gate (ignored by default): the AST oracle renders every file
/// in the corpus without panicking and deterministically. This proves the
/// serializer is total over the real grammar — no unhandled node — which is the
/// precondition for it being a trustworthy parity oracle once `parser.rss` emits
/// the same dump.
#[test]
#[ignore]
fn ast_oracle_total_over_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    let mut ok = 0usize;
    let mut empty: Vec<String> = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
        let a = ast_oracle_dump(&rel, &source);
        let b = ast_oracle_dump(&rel, &source);
        assert_eq!(a, b, "{rel}: AST oracle dump is non-deterministic");
        if a.trim().is_empty() || !a.starts_with("program\n") {
            empty.push(rel);
        } else {
            ok += 1;
        }
    }
    eprintln!(
        "\n=== ast_oracle_total_over_corpus ===\n  files: {}\n  ok: {ok}\n  degenerate: {}\n",
        files.len(),
        empty.len()
    );
    for line in empty.iter().take(20) {
        eprintln!("[degenerate] {line}");
    }
    assert!(
        empty.is_empty(),
        "{} files produced a degenerate dump",
        empty.len()
    );
}
