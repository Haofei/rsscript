use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature,
    FunctionDecl, GenericBound, GenericParam, Item, LetKind, MatchPattern, Param, Program, Stmt,
    TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

pub fn format_source(file: &str, source: &str) -> String {
    format_program(&parse_source(file, source))
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

        for (index, item) in program.items.iter().enumerate() {
            if index > 0 {
                self.out.push_str("\n\n");
            }
            match item {
                Item::Type(ty) => self.type_decl(ty),
                Item::Function(function) => self.function_decl(function),
            }
        }

        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn type_decl(&mut self, ty: &TypeDecl) {
        if ty.is_opaque {
            self.out.push_str("opaque ");
        }
        self.out.push_str(type_kind_name(ty.kind));
        self.out.push(' ');
        self.out.push_str(&ty.name);
        self.generic_params(&ty.type_params);
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
        if function.is_public {
            self.out.push_str("pub ");
        }
        if function.is_async {
            self.out.push_str("async ");
        }
        if function.is_native {
            self.out.push_str("native ");
        }
        self.out.push_str("fn ");
        self.out.push_str(&function.name);
        self.generic_params(&function.type_params);
        self.params(&function.params);
        if let Some(return_ty) = &function.return_ty {
            self.out.push_str(" -> ");
            if function.returns_fresh {
                self.out.push_str("fresh ");
            }
            self.type_ref(return_ty);
        }
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

    fn params(&mut self, params: &[Param]) {
        self.out.push('(');
        self.out.push_str(
            &params
                .iter()
                .map(format_param)
                .collect::<Vec<_>>()
                .join(", "),
        );
        self.out.push(')');
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
                self.out.push_str(match stmt.kind {
                    LetKind::Managed => "let ",
                    LetKind::Local => "local ",
                });
                self.out.push_str(&stmt.name);
                if let Some(type_annotation) = &stmt.type_annotation {
                    self.out.push_str(": ");
                    self.type_ref(type_annotation);
                }
                if let Some(value) = &stmt.value {
                    self.out.push_str(" = ");
                    self.expr(value, 0);
                }
            }
            Stmt::Return(stmt) => {
                self.out.push_str("return");
                if let Some(value) = &stmt.value {
                    self.out.push(' ');
                    self.expr(value, 0);
                }
            }
            Stmt::With(stmt) => {
                self.out.push_str("with ");
                self.expr(&stmt.resource, 0);
                self.out.push_str(" as ");
                self.out.push_str(&stmt.binding);
                self.out.push_str(" {\n");
                self.block(&stmt.body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::If(stmt) => {
                self.out.push_str("if ");
                self.expr(&stmt.condition, 0);
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
                    self.expr(condition, 0);
                } else {
                    self.out.push_str("loop");
                }
                self.out.push_str(" {\n");
                self.block(&stmt.body, indent + 1);
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::Match(stmt) => {
                self.out.push_str("match ");
                self.expr(&stmt.value, 0);
                self.out.push_str(" {\n");
                for arm in &stmt.arms {
                    self.indent(indent + 1);
                    self.match_pattern(&arm.pattern);
                    self.out.push_str(" => {\n");
                    self.block(&arm.body, indent + 2);
                    self.indent(indent + 1);
                    self.out.push_str("}\n");
                }
                self.indent(indent);
                self.out.push('}');
            }
            Stmt::Break(_) => self.out.push_str("break"),
            Stmt::Continue(_) => self.out.push_str("continue"),
            Stmt::Expr(expr) => self.expr(expr, 0),
            Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => self.out.push_str("/* unsupported */"),
        }
    }

    fn expr(&mut self, expr: &Expr, parent_precedence: u8) {
        match expr {
            Expr::Ident(name, _) | Expr::Number(name, _) => self.out.push_str(name),
            Expr::String(value, _) => self.string_literal(value),
            Expr::Binary {
                op, left, right, ..
            } => {
                let precedence = binary_precedence(*op);
                let needs_parens = precedence < parent_precedence;
                if needs_parens {
                    self.out.push('(');
                }
                self.expr(left, precedence);
                self.out.push(' ');
                self.out.push_str(binary_op_text(*op));
                self.out.push(' ');
                self.expr(right, precedence + 1);
                if needs_parens {
                    self.out.push(')');
                }
            }
            Expr::Field { base, name, .. } => {
                self.expr(base, 7);
                self.out.push('.');
                self.out.push_str(name);
            }
            Expr::Index { base, index, .. } => {
                self.expr(base, 7);
                self.out.push('[');
                self.expr(index, 0);
                self.out.push(']');
            }
            Expr::Call { callee, args, .. } => {
                self.callee(callee);
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
            Expr::Effect { effect, value, .. } => {
                self.out.push_str(data_effect_name(*effect));
                self.out.push(' ');
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr(value, 0);
                    self.out.push(')');
                } else {
                    self.expr(value, 6);
                }
            }
            Expr::Manage { value, .. } => {
                self.out.push_str("manage ");
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr(value, 0);
                    self.out.push(')');
                } else {
                    self.expr(value, 6);
                }
            }
            Expr::Spawn { value, .. } => {
                self.out.push_str("spawn ");
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr(value, 0);
                    self.out.push(')');
                } else {
                    self.expr(value, 6);
                }
            }
            Expr::Await { value, .. } => {
                self.out.push_str("await ");
                if matches!(**value, Expr::Binary { .. }) {
                    self.out.push('(');
                    self.expr(value, 0);
                    self.out.push(')');
                } else {
                    self.expr(value, 6);
                }
            }
            Expr::Try { value, .. } => {
                self.expr(value, 7);
                self.out.push('?');
            }
            Expr::Closure { params, body, .. } => {
                self.out.push('|');
                self.out.push_str(&params.join(", "));
                self.out.push_str("| {\n");
                self.block(body, 1);
                self.out.push('}');
            }
            Expr::Unknown(_) => self.out.push_str("/* unsupported */"),
        }
    }

    fn type_ref(&mut self, ty: &TypeRef) {
        self.out.push_str(&type_ref_text(ty));
    }

    fn match_pattern(&mut self, pattern: &MatchPattern) {
        match pattern {
            MatchPattern::Wildcard(_) => self.out.push('_'),
            MatchPattern::Variant {
                name,
                binding: Some(binding),
                ..
            } => {
                self.out.push_str(name);
                self.out.push('(');
                self.out.push_str(binding);
                self.out.push(')');
            }
            MatchPattern::Variant { name, .. } => self.out.push_str(name),
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

    fn callee(&mut self, callee: &Callee) {
        match callee {
            Callee::Name(name) => self.out.push_str(name),
            Callee::Qualified { namespace, name } => {
                self.out.push_str(namespace);
                self.out.push('.');
                self.out.push_str(name);
            }
        }
    }

    fn string_literal(&mut self, value: &str) {
        self.out.push('"');
        for ch in value.chars() {
            match ch {
                '\\' => self.out.push_str("\\\\"),
                '"' => self.out.push_str("\\\""),
                '\n' => self.out.push_str("\\n"),
                '\r' => self.out.push_str("\\r"),
                '\t' => self.out.push_str("\\t"),
                _ => self.out.push(ch),
            }
        }
        self.out.push('"');
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
    format!("{}: {effect}{}", param.name, type_ref_text(&param.ty))
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

fn format_generic_param(param: &GenericParam) -> String {
    match param.bound {
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

fn type_ref_text(ty: &TypeRef) -> String {
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
    if ty.is_noescape {
        format!("noescape {text}")
    } else {
        text
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

fn generic_bound_name(bound: GenericBound) -> &'static str {
    match bound {
        GenericBound::Managed => "Managed",
        GenericBound::Struct => "Struct",
        GenericBound::Resource => "Resource",
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
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual => 3,
        BinaryOp::Add | BinaryOp::Subtract => 4,
        BinaryOp::Multiply | BinaryOp::Divide => 5,
    }
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
}
