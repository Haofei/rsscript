use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature,
    FunctionDecl, GenericBound, GenericParam, Item, LetKind, MapLiteralEntry, MatchPattern,
    ObjectLiteralField, Param, Program, ProtocolImpl, Stmt, TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source_raw;

const MAX_INLINE_LITERAL_LEN: usize = 88;
const MAX_INLINE_SIGNATURE_LEN: usize = 100;

type ReceiverCallSegment<'a> = (&'a str, &'a [CallArg]);
type ReceiverCallSegments<'a> = (&'a Expr, Vec<ReceiverCallSegment<'a>>);

pub fn format_source(file: &str, source: &str) -> String {
    format_program(&parse_source_raw(file, source))
}

pub fn format_program(program: &Program) -> String {
    let mut formatter = Formatter { out: String::new() };
    formatter.program(program);
    formatter.out
}

struct Formatter {
    out: String,
}

impl Formatter {
    fn program(&mut self, program: &Program) {
        if !program.features.is_empty() {
            self.out.push_str("features: ");
            self.out.push_str(
                &program
                    .features
                    .iter()
                    .copied()
                    .map(feature_name)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            self.out.push_str("\n\n");
        }

        let mut wrote_item = false;
        for protocol in &program.protocols {
            if wrote_item {
                self.out.push_str("\n\n");
            }
            self.protocol_decl(program, &protocol.name);
            wrote_item = true;
        }

        for item in &program.items {
            if item_is_protocol_method(item, program) {
                continue;
            }
            if wrote_item {
                self.out.push_str("\n\n");
            }
            match item {
                Item::Type(ty) => self.type_decl(ty),
                Item::SumType(sum) => {
                    if sum.is_public {
                        self.out.push_str("pub ");
                    }
                    self.out.push_str("sum ");
                    self.out.push_str(&sum.name);
                    if !sum.derives.is_empty() {
                        self.out.push_str(" derives(");
                        self.out.push_str(&sum.derives.join(", "));
                        self.out.push(')');
                    }
                    self.out.push_str(" {\n");
                    for variant in &sum.variants {
                        self.out.push_str("    ");
                        self.out.push_str(&variant.name);
                        if !variant.fields.is_empty() {
                            self.out.push('(');
                            for (i, field) in variant.fields.iter().enumerate() {
                                if i > 0 {
                                    self.out.push_str(", ");
                                }
                                self.out.push_str(&field.name);
                                self.out.push_str(": ");
                                self.type_ref(&field.ty);
                            }
                            self.out.push(')');
                        }
                        self.out.push('\n');
                    }
                    self.out.push_str("}\n");
                }
                Item::TypeAlias(alias) => {
                    if alias.is_public {
                        self.out.push_str("pub ");
                    }
                    self.out.push_str("type ");
                    self.out.push_str(&alias.name);
                    self.generic_params(&alias.type_params);
                    self.out.push_str(" = ");
                    self.type_ref(&alias.target);
                    self.out.push('\n');
                }
                Item::Const(decl) => {
                    if decl.is_public {
                        self.out.push_str("pub ");
                    }
                    self.out.push_str("const ");
                    self.out.push_str(&decl.name);
                    if let Some(ty) = &decl.type_annotation {
                        self.out.push_str(": ");
                        self.type_ref(ty);
                    }
                    self.out.push_str(" = ");
                    self.expr(&decl.value, 0);
                    self.out.push('\n');
                }
                Item::Function(function) => self.function_decl(function),
                Item::Module(m) => {
                    self.out.push_str("module ");
                    self.out.push_str(&m.path.join("."));
                    self.out.push('\n');
                }
                Item::Use(u) => {
                    self.out.push_str("use ");
                    self.out.push_str(&u.path.join("."));
                    self.out.push('\n');
                }
            }
            wrote_item = true;
        }

        for protocol_impl in &program.protocol_impls {
            if wrote_item {
                self.out.push_str("\n\n");
            }
            self.protocol_impl(protocol_impl);
            wrote_item = true;
        }

        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn type_decl(&mut self, ty: &TypeDecl) {
        if ty.is_public {
            self.out.push_str("pub ");
        }
        if ty.is_opaque {
            self.out.push_str("opaque ");
        }
        self.out.push_str(type_kind_name(ty.kind));
        self.out.push(' ');
        self.out.push_str(&ty.name);
        self.generic_params(&ty.type_params);
        if !ty.derives.is_empty() {
            self.out.push_str(" derives(");
            self.out.push_str(&ty.derives.join(", "));
            self.out.push(')');
        }
        if ty.fields.is_empty() && ty.drop_body.is_none() {
            return;
        }

        self.out.push_str(" {\n");
        for field in &ty.fields {
            self.indent(1);
            self.field_decl(field);
            self.out.push('\n');
        }
        if let Some(drop_body) = &ty.drop_body {
            if !ty.fields.is_empty() {
                self.out.push('\n');
            }
            self.indent(1);
            self.out.push_str("drop {\n");
            self.block(drop_body, 2);
            self.indent(1);
            self.out.push_str("}\n");
        }
        self.out.push('}');
    }

    fn protocol_decl(&mut self, program: &Program, name: &str) {
        self.out.push_str("protocol ");
        self.out.push_str(name);
        self.out.push_str(" {\n");
        let methods = protocol_methods(program, name);
        for (index, method) in methods.iter().enumerate() {
            if index > 0 {
                self.out.push('\n');
            }
            self.indent(1);
            self.protocol_method_decl(method);
            self.out.push('\n');
        }
        self.out.push('}');
    }

    fn protocol_method_decl(&mut self, function: &FunctionDecl) {
        let visible_type_params = function
            .type_params
            .iter()
            .filter(|param| param.name != "Self")
            .cloned()
            .collect::<Vec<_>>();
        let mut prefix = String::new();
        if function.is_async {
            prefix.push_str("async ");
        }
        prefix.push_str("fn ");
        prefix.push_str(protocol_method_name(&function.name));
        prefix.push_str(&generic_params_text(&visible_type_params));
        self.function_signature(
            &prefix,
            &function.params,
            function.return_ty.as_ref(),
            function.returns_fresh,
            1,
        );
        if !function.effects.is_empty() {
            self.out.push('\n');
            self.indent(2);
            self.effects(&function.effects);
        }
    }

    fn protocol_impl(&mut self, protocol_impl: &ProtocolImpl) {
        self.out.push_str("impl ");
        self.out.push_str(&protocol_impl.protocol);
        self.out.push_str(" for ");
        self.out.push_str(&protocol_impl.type_name);
        self.out.push_str(" {\n");
        for mapping in &protocol_impl.mappings {
            self.indent(1);
            self.out.push_str(&mapping.method);
            self.out.push_str(" = ");
            self.out.push_str(&mapping.target);
            self.out.push('\n');
        }
        self.out.push('}');
    }

    fn field_decl(&mut self, field: &FieldDecl) {
        self.out.push_str(&field.name);
        self.out.push_str(": ");
        if field.is_weak {
            self.out.push_str("weak ");
        } else if field.is_handle {
            self.out.push_str("handle ");
        }
        self.type_ref(&field.ty);
    }

    fn function_decl(&mut self, function: &FunctionDecl) {
        let mut prefix = String::new();
        if function.is_public {
            prefix.push_str("pub ");
        }
        if function.is_async {
            prefix.push_str("async ");
        }
        if function.is_native {
            prefix.push_str("native ");
        }
        prefix.push_str("fn ");
        prefix.push_str(&function.name);
        prefix.push_str(&generic_params_text(&function.type_params));
        self.function_signature(
            &prefix,
            &function.params,
            function.return_ty.as_ref(),
            function.returns_fresh,
            0,
        );
        if !function.effects.is_empty() {
            self.out.push('\n');
            self.indent(1);
            self.effects(&function.effects);
        }
        if function.body.statements.is_empty() {
            return;
        }
        self.out.push_str(" {\n");
        self.block(&function.body, 1);
        self.out.push('}');
    }

    fn function_signature(
        &mut self,
        prefix: &str,
        params: &[Param],
        return_ty: Option<&TypeRef>,
        returns_fresh: bool,
        indent: usize,
    ) {
        let inline_params = format_params_text(params);
        let return_text = return_type_text(return_ty, returns_fresh);
        let inline = format!("{prefix}{inline_params}{return_text}");
        if inline.len() <= MAX_INLINE_SIGNATURE_LEN || params.len() <= 1 {
            self.out.push_str(&inline);
            return;
        }

        self.out.push_str(prefix);
        self.out.push_str("(\n");
        for param in params {
            self.indent(indent + 1);
            self.out.push_str(&format_param(param));
            self.out.push_str(",\n");
        }
        self.indent(indent);
        self.out.push(')');
        self.out.push_str(&return_text);
    }

    fn block(&mut self, block: &Block, indent: usize) {
        for statement in &block.statements {
            self.indent(indent);
            self.stmt(statement, indent);
            self.out.push('\n');
        }
    }

    fn stmt(&mut self, statement: &Stmt, indent: usize) {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.is_async {
                    self.out.push_str("async ");
                }
                self.out.push_str(match stmt.kind {
                    LetKind::Managed => "let ",
                    LetKind::Local => "local ",
                });
                if stmt.is_mut {
                    self.out.push_str("mut ");
                }
                if let Some(names) = &stmt.destructure {
                    self.out.push('(');
                    self.out.push_str(&names.join(", "));
                    self.out.push(')');
                } else {
                    self.out.push_str(&stmt.name);
                }
                if let Some(type_annotation) = &stmt.type_annotation {
                    self.out.push_str(": ");
                    self.type_ref(type_annotation);
                }
                if let Some(value) = &stmt.value {
                    self.out.push_str(" = ");
                    self.expr_at(value, 0, indent);
                }
            }
            Stmt::Return(stmt) => {
                self.out.push_str("return");
                if let Some(value) = &stmt.value {
                    self.out.push(' ');
                    self.expr_at(value, 0, indent);
                }
            }
            Stmt::With(stmt) => {
                self.out.push_str("with ");
                self.expr_at(&stmt.resource, 0, indent);
                self.out.push_str(" as ");
                self.out.push_str(&stmt.binding);
                self.out.push_str(" {\n");
                self.block(&stmt.body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::If(stmt) => {
                self.out.push_str("if ");
                self.expr_at(&stmt.condition, 0, indent);
                self.out.push_str(" {\n");
                self.block(&stmt.then_body, indent + 1);
                self.indent(indent);
                self.out.push('}');
                if let Some(else_body) = &stmt.else_body {
                    self.out.push_str(" else {\n");
                    self.block(else_body, indent + 1);
                    self.indent(indent);
                    self.out.push('}');
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.out.push_str("while ");
                    self.expr_at(condition, 0, indent);
                } else {
                    self.out.push_str("loop");
                }
                self.out.push_str(" {\n");
                self.block(&stmt.body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::For(stmt) => {
                if stmt.is_async {
                    self.out.push_str("await ");
                }
                self.out.push_str("for ");
                self.out.push_str(&stmt.binding);
                self.out.push_str(" in ");
                self.expr_at(&stmt.iterable, 0, indent);
                self.out.push_str(" {\n");
                self.block(&stmt.body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::TaskGroup(stmt) => {
                self.out.push_str("task_group {\n");
                self.block(&stmt.body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::Select(stmt) => {
                self.out.push_str("select {\n");
                for arm in &stmt.arms {
                    self.indent(indent + 1);
                    self.out.push_str(&arm.binding);
                    self.out.push_str(" = ");
                    self.expr_at(&arm.operation, 0, indent + 1);
                    self.out.push_str(" => {\n");
                    self.block(&arm.body, indent + 2);
                    self.indent(indent + 1);
                    self.out.push_str("}\n");
                }
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::Match(stmt) => {
                self.out.push_str("match ");
                if let Some(effect) = stmt.scrutinee_effect {
                    self.out.push_str(data_effect_name(effect));
                    self.out.push(' ');
                }
                self.expr_at(&stmt.value, 0, indent);
                self.out.push_str(" {\n");
                for arm in &stmt.arms {
                    self.match_arm(arm, indent);
                }
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::LetElse(stmt) => {
                self.out.push_str("let ");
                self.match_pattern(&stmt.pattern);
                self.out.push_str(" = ");
                self.expr_at(&stmt.value, 0, indent);
                self.out.push_str(" else {\n");
                self.block(&stmt.else_body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::Break(_) => self.out.push_str("break"),
            Stmt::Continue(_) => self.out.push_str("continue"),
            Stmt::Assign(stmt) => {
                self.expr_at(&stmt.target, 0, indent);
                self.out.push_str(" = ");
                self.expr_at(&stmt.value, 0, indent);
            }
            Stmt::Expr(expr) => self.expr_at(expr, 0, indent),
            Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => self.out.push_str("/* unsupported */"),
        }
    }

    fn expr(&mut self, expr: &Expr, parent_precedence: u8) {
        self.expr_at(expr, parent_precedence, 0);
    }

    fn expr_at(&mut self, expr: &Expr, parent_precedence: u8, indent: usize) {
        match expr {
            Expr::Ident(name, _) | Expr::Number(name, _) => self.out.push_str(name),
            Expr::String(value, _) => self.string_literal_at(value, indent),
            Expr::MultilineString(value, _) => self.multiline_string_literal(value, indent),
            Expr::ObjectLiteral { fields, .. } => {
                self.object_literal(fields, indent);
            }
            Expr::MapLiteral { entries, .. } => {
                self.map_literal(entries, indent);
            }
            Expr::ArrayLiteral { items, .. } => {
                self.array_literal(items, indent);
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let precedence = binary_precedence(*op);
                let needs_parens = precedence < parent_precedence;
                if needs_parens {
                    self.out.push('(');
                }
                self.expr_at(left, precedence, indent);
                self.out.push(' ');
                self.out.push_str(binary_op_text(*op));
                self.out.push(' ');
                self.expr_at(right, precedence + 1, indent);
                if needs_parens {
                    self.out.push(')');
                }
            }
            Expr::Field { base, name, .. } => {
                self.expr_at(base, 7, indent);
                self.out.push('.');
                self.out.push_str(name);
            }
            Expr::Index { base, index, .. } => {
                self.expr_at(base, 7, indent);
                self.out.push('[');
                self.expr_at(index, 0, indent);
                self.out.push(']');
            }
            Expr::Call { callee, args, .. } => {
                if self.tuple_literal(callee, args, indent) {
                    return;
                }
                if self.receiver_call_chain(expr, indent) {
                    return;
                }
                self.call_expr(callee, args, indent);
            }
            Expr::Effect { effect, value, .. } => {
                self.out.push_str(data_effect_name(*effect));
                self.out.push(' ');
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr_at(value, 0, indent);
                    self.out.push(')');
                } else {
                    self.expr_at(value, 6, indent);
                }
            }
            Expr::Manage { value, .. } => {
                self.out.push_str("manage ");
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr_at(value, 0, indent);
                    self.out.push(')');
                } else {
                    self.expr_at(value, 6, indent);
                }
            }
            Expr::Spawn { value, .. } => {
                self.out.push_str("spawn ");
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr_at(value, 0, indent);
                    self.out.push(')');
                } else {
                    self.expr_at(value, 6, indent);
                }
            }
            Expr::Await { value, .. } => {
                self.out.push_str("await ");
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr_at(value, 0, indent);
                    self.out.push(')');
                } else {
                    self.expr_at(value, 6, indent);
                }
            }
            Expr::Try { value, .. } => {
                self.expr_at(value, 7, indent);
                self.out.push('?');
            }
            Expr::Closure {
                params,
                captures,
                declared_effects,
                explicit,
                body,
                ..
            } => {
                if *explicit {
                    self.out.push_str("fn(");
                    self.out.push_str(&params.join(", "));
                    self.out.push_str(") captures(");
                    for (index, capture) in captures.iter().enumerate() {
                        if index > 0 {
                            self.out.push_str(", ");
                        }
                        self.out.push_str(data_effect_name(capture.effect));
                        self.out.push(' ');
                        self.out.push_str(&capture.name);
                    }
                    self.out.push(')');
                    if !declared_effects.is_empty() {
                        self.out.push_str(" effects(");
                        self.out.push_str(&declared_effects.join(", "));
                        self.out.push(')');
                    }
                    self.out.push_str(" {\n");
                } else {
                    self.out.push('|');
                    self.out.push_str(&params.join(", "));
                    self.out.push_str("| {\n");
                }
                self.block(body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Expr::Match {
                value,
                scrutinee_effect,
                arms,
                ..
            } => {
                self.out.push_str("match ");
                if let Some(effect) = scrutinee_effect {
                    self.out.push_str(data_effect_name(*effect));
                    self.out.push(' ');
                }
                self.expr_at(value, 0, indent);
                self.out.push_str(" {\n");
                for arm in arms {
                    self.match_arm(arm, indent);
                }
                self.indent(indent);
                self.out.push('}');
            }
            Expr::Unknown(_) => self.out.push_str("/* unsupported */"),
        }
    }

    fn object_literal(&mut self, fields: &[ObjectLiteralField], indent: usize) {
        if fields.is_empty() {
            self.out.push_str("{}");
            return;
        }
        if let Some(inline) = inline_object_literal(fields) {
            self.out.push_str(&inline);
            return;
        }
        self.out.push_str("{\n");
        for field in fields {
            self.indent(indent + 1);
            self.string_literal(&field.name);
            self.out.push_str(": ");
            self.expr_at(&field.value, 0, indent + 1);
            self.out.push_str(",\n");
        }
        self.indent(indent);
        self.out.push('}');
    }

    fn map_literal(&mut self, entries: &[MapLiteralEntry], indent: usize) {
        if entries.is_empty() {
            self.out.push_str("{}");
            return;
        }
        if let Some(inline) = inline_map_literal(entries) {
            self.out.push_str(&inline);
            return;
        }
        self.out.push_str("{\n");
        for entry in entries {
            self.indent(indent + 1);
            self.expr_at(&entry.key, 0, indent + 1);
            self.out.push_str(" => ");
            self.expr_at(&entry.value, 0, indent + 1);
            self.out.push_str(",\n");
        }
        self.indent(indent);
        self.out.push('}');
    }

    fn array_literal(&mut self, items: &[Expr], indent: usize) {
        if items.is_empty() {
            self.out.push_str("[]");
            return;
        }
        if let Some(inline) = inline_array_literal(items) {
            self.out.push_str(&inline);
            return;
        }
        self.out.push_str("[\n");
        for item in items {
            self.indent(indent + 1);
            self.expr_at(item, 0, indent + 1);
            self.out.push_str(",\n");
        }
        self.indent(indent);
        self.out.push(']');
    }

    /// Render a synthetic tuple constructor `__TupleN(item0: .., item1: ..)` back
    /// as `(.., ..)`. Returns `false` if the call is not a well-formed tuple
    /// literal, leaving it to the ordinary call renderer.
    fn tuple_literal(&mut self, callee: &Callee, args: &[CallArg], indent: usize) -> bool {
        let Callee::Name(name) = callee else {
            return false;
        };
        let Some(arity) = tuple_arity_of(name) else {
            return false;
        };
        if args.len() != arity
            || !args
                .iter()
                .enumerate()
                .all(|(index, arg)| arg.name.as_deref() == Some(format!("item{index}").as_str()))
        {
            return false;
        }
        self.out.push('(');
        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                self.out.push_str(", ");
            }
            self.expr_at(&arg.value, 0, indent);
        }
        self.out.push(')');
        true
    }

    fn call_expr(&mut self, callee: &Callee, args: &[CallArg], indent: usize) {
        if let Some(inline) = inline_call_expr(callee, args) {
            if inline.len() <= MAX_INLINE_SIGNATURE_LEN {
                self.out.push_str(&inline);
                return;
            }
        }

        if args.len() <= 1 {
            self.callee_at(callee, indent);
            self.out.push('(');
            if let Some(arg) = args.first() {
                self.call_arg(arg, indent);
            }
            self.out.push(')');
            return;
        }

        self.callee_at(callee, indent);
        self.out.push_str("(\n");
        for arg in args {
            self.indent(indent + 1);
            self.call_arg(arg, indent + 1);
            self.out.push_str(",\n");
        }
        self.indent(indent);
        self.out.push(')');
    }

    fn call_arg(&mut self, arg: &CallArg, indent: usize) {
        if let Some(name) = &arg.name {
            self.out.push_str(name);
            self.out.push_str(": ");
        }
        self.expr_at(&arg.value, 0, indent);
    }

    fn receiver_call_chain(&mut self, expr: &Expr, indent: usize) -> bool {
        let Some(inline) = inline_expr(expr) else {
            return false;
        };
        if inline.len() <= MAX_INLINE_SIGNATURE_LEN {
            return false;
        }
        let Some((base, segments)) = receiver_call_segments(expr) else {
            return false;
        };
        self.expr_at(base, 7, indent);
        for (method, args) in segments {
            self.out.push('\n');
            self.indent(indent + 1);
            self.out.push('.');
            self.out.push_str(method);
            self.out.push('(');
            self.out.push_str(
                &args
                    .iter()
                    .map(format_call_arg)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            self.out.push(')');
        }
        true
    }

    fn type_ref(&mut self, ty: &TypeRef) {
        self.out.push_str(&type_ref_text(ty));
    }

    fn match_arm(&mut self, arm: &crate::syntax::ast::MatchArm, indent: usize) {
        self.indent(indent + 1);
        self.match_pattern(&arm.pattern);
        if let Some(guard) = &arm.guard {
            self.out.push_str(" if ");
            self.expr_at(guard, 0, indent + 1);
        }
        self.out.push_str(" => {\n");
        self.block(&arm.body, indent + 2);
        self.indent(indent + 1);
        self.out.push_str("}\n");
    }

    fn match_pattern(&mut self, pattern: &MatchPattern) {
        match pattern {
            MatchPattern::Binding { name, .. } => self.out.push_str(name),
            MatchPattern::Wildcard(_) => self.out.push('_'),
            MatchPattern::Literal { value, .. } => match value {
                crate::syntax::ast::MatchLiteral::Int(value) => self.out.push_str(value),
                crate::syntax::ast::MatchLiteral::String(value) => {
                    self.out.push('"');
                    self.out.push_str(value);
                    self.out.push('"');
                }
                crate::syntax::ast::MatchLiteral::Bool(value) => {
                    self.out.push_str(if *value { "true" } else { "false" });
                }
            },
            MatchPattern::Variant {
                name,
                binding: Some(binding),
                ..
            } => {
                self.out.push_str(name);
                self.out.push('(');
                self.match_pattern(binding);
                self.out.push(')');
            }
            MatchPattern::Variant { name, .. } => self.out.push_str(name),
            MatchPattern::Struct {
                name,
                fields,
                has_rest,
                ..
            } if tuple_arity_of(name) == Some(fields.len())
                && !has_rest
                && fields.iter().enumerate().all(|(index, field)| {
                    field.name == format!("item{index}") && field.effect.is_none()
                }) =>
            {
                // Render a synthetic tuple struct pattern back as `(p0, p1, ..)`.
                self.out.push('(');
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        self.out.push_str(", ");
                    }
                    if field.ignored {
                        self.out.push('_');
                    } else if let Some(pattern) = &field.pattern {
                        self.match_pattern(pattern);
                    } else if let Some(binding) = &field.binding {
                        self.out.push_str(binding);
                    }
                }
                self.out.push(')');
            }
            MatchPattern::Struct {
                name,
                fields,
                has_rest,
                ..
            } => {
                self.out.push_str(name);
                self.out.push_str(" { ");
                let mut first = true;
                for field in fields {
                    if !first {
                        self.out.push_str(", ");
                    }
                    first = false;
                    self.out.push_str(&field.name);
                    let needs_rhs = field.ignored
                        || field.effect.is_some()
                        || field.pattern.is_some()
                        || field.binding.as_deref() != Some(field.name.as_str());
                    if needs_rhs {
                        self.out.push_str(": ");
                        if let Some(effect) = field.effect {
                            self.out.push_str(data_effect_name(effect));
                            self.out.push(' ');
                        }
                        if field.ignored {
                            self.out.push('_');
                        } else if let Some(pattern) = &field.pattern {
                            self.match_pattern(pattern);
                        } else if let Some(binding) = &field.binding {
                            self.out.push_str(binding);
                        }
                    }
                }
                if *has_rest {
                    if !first {
                        self.out.push_str(", ");
                    }
                    self.out.push_str("..");
                }
                self.out.push_str(" }");
            }
            MatchPattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => {
                self.out.push('[');
                let mut first = true;
                for pattern in prefix {
                    if !first {
                        self.out.push_str(", ");
                    }
                    first = false;
                    self.match_pattern(pattern);
                }
                if let Some(rest_binding) = rest {
                    if !first {
                        self.out.push_str(", ");
                    }
                    first = false;
                    self.out.push_str("..");
                    if let Some(name) = rest_binding {
                        self.out.push_str(name);
                    }
                }
                for pattern in suffix {
                    if !first {
                        self.out.push_str(", ");
                    }
                    first = false;
                    self.match_pattern(pattern);
                }
                self.out.push(']');
            }
        }
    }

    fn generic_params(&mut self, params: &[GenericParam]) {
        if params.is_empty() {
            return;
        }
        self.out.push('<');
        self.out.push_str(
            &params
                .iter()
                .map(format_generic_param)
                .collect::<Vec<_>>()
                .join(", "),
        );
        self.out.push('>');
    }

    fn effects(&mut self, effects: &[EffectDecl]) {
        self.out.push_str("effects(");
        self.out.push_str(
            &effects
                .iter()
                .map(format_effect)
                .collect::<Vec<_>>()
                .join(", "),
        );
        self.out.push(')');
    }

    fn callee_at(&mut self, callee: &Callee, indent: usize) {
        match callee {
            Callee::Name(name) => self.out.push_str(name),
            Callee::Qualified { namespace, name } => {
                self.out.push_str(namespace);
                self.out.push('.');
                self.out.push_str(name);
            }
            Callee::ReceiverCall {
                receiver,
                method,
                effect,
            } => {
                if *effect != DataEffect::Read {
                    self.out.push_str(effect.as_str());
                    self.out.push(' ');
                }
                self.expr_at(receiver, 0, indent);
                self.out.push('.');
                self.out.push_str(method);
            }
        }
    }

    fn string_literal(&mut self, value: &str) {
        self.string_literal_at(value, 0);
    }

    fn string_literal_at(&mut self, value: &str, indent: usize) {
        if value.contains('\n') && !value.contains("\"\"\"") {
            self.multiline_string_literal(value, indent);
            return;
        }
        self.out.push('"');
        self.out.push_str(value);
        self.out.push('"');
    }

    fn multiline_string_literal(&mut self, value: &str, indent: usize) {
        self.out.push_str("\"\"\"");
        let mut lines = value.split('\n').peekable();
        if let Some(first) = lines.next() {
            self.out.push_str(first);
        }
        while let Some(line) = lines.next() {
            self.out.push('\n');
            if lines.peek().is_none() && line.is_empty() {
                self.indent(indent);
            }
            self.out.push_str(line);
        }
        self.out.push_str("\"\"\"");
    }

    fn indent(&mut self, indent: usize) {
        for _ in 0..indent {
            self.out.push_str("    ");
        }
    }
}

fn format_param(param: &Param) -> String {
    let effect = param
        .effect
        .map(data_effect_name)
        .map(|effect| format!("{effect} "))
        .unwrap_or_default();
    let default = param
        .default
        .as_ref()
        .map(|expr| format!(" = {}", expr_text(expr)))
        .unwrap_or_default();
    format!(
        "{}: {effect}{}{default}",
        param.name,
        type_ref_text(&param.ty)
    )
}

/// Render a standalone expression to RSScript source (used for parameter
/// defaults), reusing the buffer-based expression formatter.
fn expr_text(expr: &Expr) -> String {
    let mut formatter = Formatter { out: String::new() };
    formatter.expr(expr, 0);
    formatter.out
}

fn format_params_text(params: &[Param]) -> String {
    format!(
        "({})",
        params
            .iter()
            .map(format_param)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn generic_params_text(params: &[GenericParam]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            params
                .iter()
                .map(format_generic_param)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn return_type_text(return_ty: Option<&TypeRef>, returns_fresh: bool) -> String {
    let Some(return_ty) = return_ty else {
        return String::new();
    };
    let fresh = if returns_fresh && !type_ref_contains_fresh(return_ty) {
        "fresh "
    } else {
        ""
    };
    format!(" -> {fresh}{}", type_ref_text(return_ty))
}

fn type_ref_contains_fresh(ty: &TypeRef) -> bool {
    ty.is_fresh
        || ty.args.iter().any(type_ref_contains_fresh)
        || ty.fn_params.iter().any(type_ref_contains_fresh)
        || ty.fn_return.as_deref().is_some_and(type_ref_contains_fresh)
}

fn format_call_arg(arg: &CallArg) -> String {
    let mut formatter = Formatter { out: String::new() };
    if let Some(name) = &arg.name {
        formatter.out.push_str(name);
        formatter.out.push_str(": ");
    }
    formatter.expr(&arg.value, 0);
    formatter.out
}

fn inline_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) | Expr::Number(name, _) => Some(name.clone()),
        Expr::String(value, _) => Some(quoted_string(value)),
        Expr::MultilineString(value, _) => Some(format!("\"\"\"{value}\"\"\"")),
        Expr::ObjectLiteral { fields, .. } => inline_object_literal(fields),
        Expr::MapLiteral { entries, .. } => inline_map_literal(entries),
        Expr::ArrayLiteral { items, .. } => inline_array_literal(items),
        Expr::Binary {
            op, left, right, ..
        } => Some(format!(
            "{} {} {}",
            inline_expr(left)?,
            binary_op_text(*op),
            inline_expr(right)?
        )),
        Expr::Field { base, name, .. } => Some(format!("{}.{}", inline_expr(base)?, name)),
        Expr::Index { base, index, .. } => {
            Some(format!("{}[{}]", inline_expr(base)?, inline_expr(index)?))
        }
        Expr::Call { callee, args, .. } => {
            if let Some(tuple) = inline_tuple_literal(callee, args) {
                return Some(tuple);
            }
            let args = args
                .iter()
                .map(inline_call_arg)
                .collect::<Option<Vec<_>>>()?
                .join(", ");
            Some(format!("{}({args})", inline_callee(callee)?))
        }
        Expr::Try { value, .. } => Some(format!("{}?", inline_expr(value)?)),
        Expr::Effect { effect, value, .. } => Some(format!(
            "{} {}",
            data_effect_name(*effect),
            inline_expr(value)?
        )),
        Expr::Manage { value, .. } => Some(format!("manage {}", inline_expr(value)?)),
        Expr::Spawn { value, .. } => Some(format!("spawn {}", inline_expr(value)?)),
        Expr::Await { value, .. } => Some(format!("await {}", inline_expr(value)?)),
        Expr::Closure { .. } | Expr::Match { .. } | Expr::Unknown(_) => None,
    }
}

/// Render a synthetic tuple constructor `__TupleN(item0: .., ..)` inline as
/// `(.., ..)`, or `None` if the call is not a well-formed tuple literal.
fn inline_tuple_literal(callee: &Callee, args: &[CallArg]) -> Option<String> {
    let Callee::Name(name) = callee else {
        return None;
    };
    let arity = tuple_arity_of(name)?;
    if args.len() != arity
        || !args
            .iter()
            .enumerate()
            .all(|(index, arg)| arg.name.as_deref() == Some(format!("item{index}").as_str()))
    {
        return None;
    }
    let inner = args
        .iter()
        .map(|arg| inline_expr(&arg.value))
        .collect::<Option<Vec<_>>>()?
        .join(", ");
    Some(format!("({inner})"))
}

fn inline_call_arg(arg: &CallArg) -> Option<String> {
    let value = inline_expr(&arg.value)?;
    Some(match &arg.name {
        Some(name) => format!("{name}: {value}"),
        None => value,
    })
}

fn inline_call_expr(callee: &Callee, args: &[CallArg]) -> Option<String> {
    let args = args
        .iter()
        .map(inline_call_arg)
        .collect::<Option<Vec<_>>>()?
        .join(", ");
    Some(format!("{}({args})", inline_callee(callee)?))
}

fn inline_callee(callee: &Callee) -> Option<String> {
    match callee {
        Callee::Name(name) => Some(name.clone()),
        Callee::Qualified { namespace, name } => Some(format!("{namespace}.{name}")),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => {
            let prefix = if *effect == DataEffect::Read {
                String::new()
            } else {
                format!("{} ", effect.as_str())
            };
            Some(format!("{prefix}{}.{}", inline_expr(receiver)?, method))
        }
    }
}

fn receiver_call_segments(expr: &Expr) -> Option<ReceiverCallSegments<'_>> {
    let mut current = expr;
    let mut segments = Vec::new();
    while let Expr::Call {
        callee:
            Callee::ReceiverCall {
                receiver,
                method,
                effect,
            },
        args,
        ..
    } = current
    {
        if *effect != DataEffect::Read {
            return None;
        }
        segments.push((method.as_str(), args.as_slice()));
        current = receiver;
    }
    if segments.len() < 2 {
        return None;
    }
    segments.reverse();
    Some((current, segments))
}

fn inline_object_literal(fields: &[ObjectLiteralField]) -> Option<String> {
    let parts = fields
        .iter()
        .map(|field| {
            Some(format!(
                "{}: {}",
                quoted_string(&field.name),
                inline_expr(&field.value)?
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    inline_literal(format!("{{{}}}", parts.join(", ")))
}

fn inline_map_literal(entries: &[MapLiteralEntry]) -> Option<String> {
    let parts = entries
        .iter()
        .map(|entry| {
            Some(format!(
                "{} => {}",
                inline_expr(&entry.key)?,
                inline_expr(&entry.value)?
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    inline_literal(format!("{{{}}}", parts.join(", ")))
}

fn inline_array_literal(items: &[Expr]) -> Option<String> {
    let parts = items.iter().map(inline_expr).collect::<Option<Vec<_>>>()?;
    inline_literal(format!("[{}]", parts.join(", ")))
}

fn inline_literal(value: String) -> Option<String> {
    (value.len() <= MAX_INLINE_LITERAL_LEN).then_some(value)
}

fn quoted_string(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn format_generic_param(param: &GenericParam) -> String {
    match param.bound.as_ref() {
        Some(bound) => format!("{}: {}", param.name, generic_bound_name(bound)),
        None => param.name.clone(),
    }
}

fn format_effect(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(name) => format!("retains({name})"),
    }
}

/// The tuple arity encoded in a synthetic `__TupleN` name (`N >= 2`), if any.
fn tuple_arity_of(name: &str) -> Option<usize> {
    name.strip_prefix("__Tuple")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|arity| *arity >= 2)
}

fn type_ref_text(ty: &TypeRef) -> String {
    // Render synthetic tuple types `__TupleN<...>` back as `(A, B, ...)`.
    if let Some(arity) = tuple_arity_of(&ty.name)
        && ty.args.len() == arity
    {
        let inner = ty
            .args
            .iter()
            .map(type_ref_text)
            .collect::<Vec<_>>()
            .join(", ");
        let tuple = format!("({inner})");
        return if ty.is_fresh {
            format!("fresh {tuple}")
        } else {
            tuple
        };
    }
    let text = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_text)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_text(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(type_ref_text)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let name = if ty.is_noescape {
        format!("noescape {text}")
    } else if ty.is_owned {
        format!("owned {text}")
    } else {
        text
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

fn feature_name(feature: FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}

fn type_kind_name(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Resource => "resource",
    }
}

fn generic_bound_name(bound: &GenericBound) -> &str {
    match bound {
        GenericBound::Managed => "Managed",
        GenericBound::Struct => "Struct",
        GenericBound::Resource => "Resource",
        GenericBound::Protocol(name) => name,
    }
}

fn data_effect_name(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn binary_op_text(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Modulo => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight => ">>",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::LogicalAnd => "&&",
        BinaryOp::LogicalOr => "||",
    }
}

fn binary_precedence(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::LogicalOr => 1,
        BinaryOp::LogicalAnd => 2,
        BinaryOp::BitOr => 3,
        BinaryOp::BitXor => 4,
        BinaryOp::BitAnd => 5,
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual => 6,
        BinaryOp::ShiftLeft | BinaryOp::ShiftRight => 7,
        BinaryOp::Add | BinaryOp::Subtract => 8,
        BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Modulo => 9,
    }
}

fn item_is_protocol_method(item: &Item, program: &Program) -> bool {
    let Item::Function(function) = item else {
        return false;
    };
    program
        .protocols
        .iter()
        .any(|protocol| is_protocol_method(function, &protocol.name))
}

fn protocol_methods<'a>(program: &'a Program, protocol: &str) -> Vec<&'a FunctionDecl> {
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

fn is_protocol_method(function: &FunctionDecl, protocol: &str) -> bool {
    function
        .name
        .rsplit_once('.')
        .is_some_and(|(namespace, _)| namespace == protocol)
}

fn protocol_method_name(name: &str) -> &str {
    name.rsplit_once('.')
        .map(|(_, method)| method)
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn formats_core_surface_deterministically() {
        let source = r#"features:   local
struct   Session {
owner:weak User
cache: handle Map<String,String>
}
fn  save(image:read Image,path:read Path)->Result<Unit, IOError>
effects(retains(image), no_panic){
local tmp=Image.clone(image:read image)?
Image.save(image:read tmp,path:read path)
return Unit
}
"#;

        assert_eq!(
            format_source("fmt.rss", source),
            r#"features: local

struct Session {
    owner: weak User
    cache: handle Map<String, String>
}

fn save(image: read Image, path: read Path) -> Result<Unit, IOError>
    effects(retains(image), no_panic) {
    local tmp = Image.clone(image: read image)?
    Image.save(image: read tmp, path: read path)
    return Unit
}
"#
        );
    }

    #[test]
    fn formats_effectful_struct_match_patterns() {
        let source = r#"fn inspect(expr:read Expr)->String{
match read expr{
Call{callee,args} if List.is_empty(list:read args)=>{
return callee
}
Binary{left:_,right:read rhs,..}=>{
return rhs
}
_=>{
return ""
}
}
}
"#;

        assert_eq!(
            format_source("pattern.rss", source),
            r#"fn inspect(expr: read Expr) -> String {
    match read expr {
        Call { callee, args } if List.is_empty(list: read args) => {
            return callee
        }
        Binary { left: _, right: read rhs, .. } => {
            return rhs
        }
        _ => {
            return ""
        }
    }
}
"#
        );
    }

    #[test]
    fn preserves_native_function_declarations() {
        let source = r#"features: native
native   fn Host.emit(message:read String)->Unit
effects(native)
"#;

        assert_eq!(
            format_source("native.rssi", source),
            r#"features: native

native fn Host.emit(message: read String) -> Unit
    effects(native)
"#
        );
    }

    #[test]
    fn preserves_associated_constant_names() {
        // The formatter parses raw (no desugaring), so a type-associated const
        // keeps its dotted source form rather than the flattened lowered name.
        let source = "const Device.DEFAULT: String = \"cpu\"\n";
        assert_eq!(format_source("assoc.rss", source), source);
    }

    #[test]
    fn preserves_parameter_default_values() {
        let source = "fn f(a: Int, b: Int = 5) -> Int {\n    return a + b\n}\n";
        // Reformatting must preserve the default value (idempotent + lossless).
        let formatted = format_source("defaults.rss", source);
        assert!(
            formatted.contains("b: Int = 5"),
            "formatter dropped the parameter default:\n{formatted}"
        );
        assert_eq!(format_source("defaults.rss", &formatted), formatted);
    }

    #[test]
    fn preserves_noescape_function_parameter_types() {
        let source = r#"fn apply(callback:noescape Fn())->Unit {
return Unit
}
"#;

        assert_eq!(
            format_source("noescape.rss", source),
            r#"fn apply(callback: noescape Fn()) -> Unit {
    return Unit
}
"#
        );
    }

    #[test]
    fn preserves_function_type_parameter_and_return_types() {
        let source = r#"fn schedule(callback:Fn(Int)->Result<String,BuildError>)->Unit {
return Unit
}
"#;

        assert_eq!(
            format_source("fn-type.rss", source),
            r#"fn schedule(callback: Fn(Int) -> Result<String, BuildError>) -> Unit {
    return Unit
}
"#
        );
    }

    #[test]
    fn formats_receiver_calls_without_default_read_noise() {
        let source = r#"fn ok(path_text:read String)->Bool{
let ok=path_text.safe_relative().is_ok()
let mut values=List<String>.new()
mut values.push("done")
return ok
}
"#;

        assert_eq!(
            format_source("receiver-format.rss", source),
            r#"fn ok(path_text: read String) -> Bool {
    let ok = path_text.safe_relative().is_ok()
    let mut values = List<String>.new()
    mut values.push("done")
    return ok
}
"#
        );
    }

    #[test]
    fn formats_large_json_literals_for_review() {
        let source = r#"fn tool_specs()->fresh JsonValue{
let specs:List<JsonValue>=[{"type":"function","function":{"name":"read_file","description":"Read a UTF-8 text file for the agent.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"Relative path to read."}},"required":["path"],"additionalProperties":false}}},{"type":"function","function":{"name":"list_files","description":"List files under a relative directory.","parameters":{"type":"object","properties":{"path":{"type":"string"},"max_results":{"type":"integer"}},"required":["path"],"additionalProperties":false}}}]
return {"ok":true,"tools":specs.to_json_values()}
}
"#;

        assert_eq!(
            format_source("json-format.rss", source),
            r#"fn tool_specs() -> fresh JsonValue {
    let specs: List<JsonValue> = [
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a UTF-8 text file for the agent.",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "Relative path to read."}},
                    "required": ["path"],
                    "additionalProperties": false,
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "list_files",
                "description": "List files under a relative directory.",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}, "max_results": {"type": "integer"}},
                    "required": ["path"],
                    "additionalProperties": false,
                },
            },
        },
    ]
    return {"ok": true, "tools": specs.to_json_values()}
}
"#
        );
    }

    #[test]
    fn formats_long_function_signatures_for_review() {
        let source = r#"fn collect_files_recursive(root:read Path,display_root_text:read String,results:mut List<String>,max_results:Int)->Unit{
return Unit
}
fn short(path:read Path)->fresh JsonValue{
return {"ok":true}
}
"#;

        assert_eq!(
            format_source("signature-format.rss", source),
            r#"fn collect_files_recursive(
    root: read Path,
    display_root_text: read String,
    results: mut List<String>,
    max_results: Int,
) -> Unit {
    return Unit
}

fn short(path: read Path) -> fresh JsonValue {
    return {"ok": true}
}
"#
        );
    }

    #[test]
    fn formats_long_receiver_chains_for_review() {
        let source = r#"fn message(workspace_root:read String)->fresh String{
let short="a".concat("b")
let reason="writes are confined to the workspace root `".concat(workspace_root).concat("` (no absolute paths or `..`)")
return reason
}
"#;

        assert_eq!(
            format_source("receiver-chain-format.rss", source),
            r#"fn message(workspace_root: read String) -> fresh String {
    let short = "a".concat("b")
    let reason = "writes are confined to the workspace root `"
        .concat(workspace_root)
        .concat("` (no absolute paths or `..`)")
    return reason
}
"#
        );
    }

    #[test]
    fn formats_long_calls_and_constructors_for_review() {
        let source = r#"features: async
async fn main(config:read AgentConfig)->Result<Unit,HttpError>{
let http=await Http.post_json_bearer_retry_async(url:read config.endpoint,body:read request_body,token:read config.api_key,timeout_ms:config.request_timeout_ms,attempts:config.max_attempts,backoff_ms:config.backoff_ms)?
return Ok(Unit)
}
fn config()->fresh AgentConfig{
return AgentConfig(model:"AGENT_MODEL".env_or("gpt-5.5:medium"),endpoint:"AGENT_ENDPOINT".env_or("http://localhost:8080/v1/chat/completions").to_url(),api_key:"AGENT_API_KEY".env_or("test_key"),max_steps:env_min_int("AGENT_MAX_STEPS",8,1),max_tool_calls:env_min_int("AGENT_MAX_TOOL_CALLS",8,1),request_timeout_ms:env_min_int("AGENT_TIMEOUT_MS",60000,1))
}
"#;

        assert_eq!(
            format_source("call-format.rss", source),
            r#"features: async

async fn main(config: read AgentConfig) -> Result<Unit, HttpError> {
    let http = await Http.post_json_bearer_retry_async(
        url: read config.endpoint,
        body: read request_body,
        token: read config.api_key,
        timeout_ms: config.request_timeout_ms,
        attempts: config.max_attempts,
        backoff_ms: config.backoff_ms,
    )?
    return Ok(Unit)
}

fn config() -> fresh AgentConfig {
    return AgentConfig(
        model: "AGENT_MODEL".env_or("gpt-5.5:medium"),
        endpoint: "AGENT_ENDPOINT".env_or("http://localhost:8080/v1/chat/completions").to_url(),
        api_key: "AGENT_API_KEY".env_or("test_key"),
        max_steps: env_min_int("AGENT_MAX_STEPS", 8, 1),
        max_tool_calls: env_min_int("AGENT_MAX_TOOL_CALLS", 8, 1),
        request_timeout_ms: env_min_int("AGENT_TIMEOUT_MS", 60000, 1),
    )
}
"#
        );
    }

    #[test]
    fn formats_closure_bodies_at_expression_indent() {
        let source = r#"fn count_next(values:read List<Int>)->Int{
return values.map(|item|{
return item + 1
}).len()
}
"#;

        assert_eq!(
            format_source("closure-indent.rss", source),
            r#"fn count_next(values: read List<Int>) -> Int {
    return values.map(|item| {
        return item + 1
    }).len()
}
"#
        );
    }

    #[test]
    fn preserves_multiline_string_literals() {
        let source = "fn prompt() -> String {\nreturn \"\"\"First line\nSecond line\"\"\"\n}\n";

        assert_eq!(
            format_source("multiline-string.rss", source),
            "fn prompt() -> String {\n    return \"\"\"First line\nSecond line\"\"\"\n}\n"
        );
    }

    #[test]
    fn preserves_multiline_string_backslashes_as_raw_text() {
        let source = "fn prompt() -> String {\nreturn \"\"\"literal \\n text\"\"\"\n}\n";

        assert_eq!(
            format_source("multiline-string-raw.rss", source),
            "fn prompt() -> String {\n    return \"\"\"literal \\n text\"\"\"\n}\n"
        );
    }

    #[test]
    fn preserves_escaped_string_literals_without_reescaping() {
        let source = "fn text() -> String {\nreturn \"hello\\\\n\"\n}\n";

        assert_eq!(
            format_source("escaped-string.rss", source),
            "fn text() -> String {\n    return \"hello\\\\n\"\n}\n"
        );
    }

    #[test]
    fn preserves_fresh_inside_generic_type_arguments() {
        let source = r#"fn value_at(root:read JsonValue,index:Int)->Result<fresh JsonValue,JsonError>{
return root.array_get(index)
}
"#;

        assert_eq!(
            format_source("fresh-generic-format.rss", source),
            r#"fn value_at(root: read JsonValue, index: Int) -> Result<fresh JsonValue, JsonError> {
    return root.array_get(index)
}
"#
        );
    }

    #[test]
    fn preserves_protocol_declarations_and_impl_mappings() {
        let source = r#"protocol Writer {
fn write(self:mut Self,message:read String)->Unit
effects(retains(message))
}
struct BufferWriter
fn BufferWriter.write(self:mut BufferWriter,message:read String)->Unit
effects(retains(message)){
Log.write(message:read message)
}
impl Writer for BufferWriter {
write=BufferWriter.write
}
fn write_line<W:Writer>(writer:mut W,message:read String)->Unit{
Writer.write(self:mut writer,message:read message)
}
"#;

        assert_eq!(
            format_source("protocol.rss", source),
            r#"protocol Writer {
    fn write(self: mut Self, message: read String) -> Unit
        effects(retains(message))
}

struct BufferWriter

fn BufferWriter.write(self: mut BufferWriter, message: read String) -> Unit
    effects(retains(message)) {
    Log.write(message: read message)
}

fn write_line<W: Writer>(writer: mut W, message: read String) -> Unit {
    Writer.write(self: mut writer, message: read message)
}

impl Writer for BufferWriter {
    write = BufferWriter.write
}
"#
        );
    }

    #[test]
    fn renders_list_slice_patterns() {
        let source = r#"features: local

fn describe(xs:read List<Int>)->Int{
match read xs{
[]=>{return 0}
[only]=>{return only}
[first,..rest]=>{return first}
[..init,last]=>{return last}
[a,..mid,z]=>{return a}
_=>{return -1}
}
}
"#;

        assert_eq!(
            format_source("list.rss", source),
            r#"features: local

fn describe(xs: read List<Int>) -> Int {
    match read xs {
        [] => {
            return 0
        }
        [only] => {
            return only
        }
        [first, ..rest] => {
            return first
        }
        [..init, last] => {
            return last
        }
        [a, ..mid, z] => {
            return a
        }
        _ => {
            return -1
        }
    }
}
"#
        );
    }

    #[test]
    fn renders_tuple_literals_types_patterns_and_destructuring() {
        let source = r#"fn first(p:read (Int,String))->Int{
match read p{
(0,_)=>{return 0}
(n,_)=>{return n}
}
}

fn main()->Unit{
let (a,b)=(5,"x")
Log.write(message:read b)
Log.write(message:read String.from_int(value:first(p:read (a,b))))
return Unit
}
"#;

        assert_eq!(
            format_source("tuples.rss", source),
            r#"fn first(p: read (Int, String)) -> Int {
    match read p {
        (0, _) => {
            return 0
        }
        (n, _) => {
            return n
        }
    }
}

fn main() -> Unit {
    let (a, b) = (5, "x")
    Log.write(message: read b)
    Log.write(message: read String.from_int(value: first(p: read (a, b))))
    return Unit
}
"#
        );
    }
}
