use std::collections::{HashMap, HashSet};

use crate::ast::{
    FileMode, FunctionDecl, Program, TypeKind, find_matching, ident_name, parse_program,
};
use crate::diagnostic::Diagnostic;
use crate::lexer::{Token, TokenKind, lex};

pub fn analyze_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let program = parse_program(&tokens);
    let mut analyzer = Analyzer {
        tokens: &tokens,
        program,
        diagnostics: Vec::new(),
    };
    analyzer.run();
    analyzer.diagnostics
}

struct Analyzer<'a> {
    tokens: &'a [Token],
    program: Program,
    diagnostics: Vec<Diagnostic>,
}

impl Analyzer<'_> {
    fn run(&mut self) {
        self.check_file_mode_present();
        self.check_signature_explicitness();
        self.check_resource_fields();
        self.check_mode_violations();
        self.check_calls();
        self.check_functions();
        self.check_operator_overload_attempts();
    }

    fn check_file_mode_present(&mut self) {
        if self.program.mode.is_none() {
            let span = self.tokens.first().map(|token| token.span.clone()).unwrap();
            self.diagnostics.push(
                Diagnostic::error(
                    "RS0001",
                    "RSScript files must declare exactly one file mode.",
                    span,
                    "missing mode",
                )
                .with_fix(
                    "add_mode",
                    "Add `mode: managed` or `mode: uses-local`.",
                    "manual",
                ),
            );
        }
    }

    fn check_signature_explicitness(&mut self) {
        for function in self.program.functions.values() {
            if function
                .return_type
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                let span = self
                    .tokens
                    .get(function.body_start.saturating_sub(1))
                    .map(|token| token.span.clone())
                    .unwrap_or_else(|| self.tokens[0].span.clone());
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0002",
                        format!("function `{}` must declare an explicit return type.", function.name),
                        span,
                        "missing return type",
                    )
                    .with_cause("Public APIs must not rely on inference; this checker applies the canonical rule to all functions.")
                    .with_fix("add_return_type", "Add `-> Unit` or another explicit return type.", "manual"),
                );
            }

            for param in &function.params {
                if param.type_name.is_empty() {
                    let span = self
                        .tokens
                        .get(function.body_start.saturating_sub(1))
                        .map(|token| token.span.clone())
                        .unwrap_or_else(|| self.tokens[0].span.clone());
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0003",
                            format!(
                                "parameter `{}` in `{}` must declare an explicit type.",
                                param.name, function.name
                            ),
                            span,
                            "missing parameter type",
                        )
                        .with_fix(
                            "add_parameter_type",
                            "Add an explicit parameter type.",
                            "manual",
                        ),
                    );
                }
            }

            for effect in &function.effects {
                let valid = effect == "no_panic"
                    || effect == "noalloc"
                    || effect == "no_block"
                    || effect == "pure"
                    || effect == "unsafe"
                    || effect == "native"
                    || effect.starts_with("retains(");
                if !valid {
                    let span = self
                        .tokens
                        .get(function.body_start.saturating_sub(1))
                        .map(|token| token.span.clone())
                        .unwrap_or_else(|| self.tokens[0].span.clone());
                    self.diagnostics.push(Diagnostic::warning(
                        "RS0004",
                        format!("unknown effect `{effect}` in `{}`.", function.name),
                        span,
                        "unknown effect",
                    ));
                }
            }
        }
    }

    fn check_resource_fields(&mut self) {
        let resources: HashSet<String> = self
            .program
            .types
            .values()
            .filter(|decl| decl.kind == TypeKind::Resource)
            .map(|decl| decl.name.clone())
            .collect();

        for decl in self.program.types.values() {
            if decl.kind == TypeKind::Resource {
                continue;
            }
            for field in &decl.fields {
                if resources.contains(&field.type_name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0701",
                            format!("resource `{}` cannot be stored in `{}`.", field.type_name, decl.name),
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

    fn check_mode_violations(&mut self) {
        let Some((mode, _)) = self.program.mode else {
            return;
        };
        if mode != FileMode::Managed {
            return;
        }

        for token in self.tokens {
            let violation = if token.is_ident_text("local") {
                Some("`local` requires `mode: uses-local`.")
            } else if token.is_ident_text("manage") {
                Some("`manage` requires `mode: uses-local`.")
            } else if token.is_ident_text("take") {
                Some("`take` requires `mode: uses-local`.")
            } else if token.is_ident_text("ResourcePool") {
                Some("`ResourcePool<T>` requires `mode: uses-local`.")
            } else {
                None
            };

            if let Some(summary) = violation {
                self.diagnostics.push(
                    Diagnostic::error("RS0101", summary, token.span.clone(), "mode violation")
                        .with_fix(
                            "change_mode",
                            "Change the file declaration to `mode: uses-local`.",
                            "manual",
                        ),
                );
            }
        }
    }

    fn check_calls(&mut self) {
        let mut i = 0;
        while i + 1 < self.tokens.len() {
            let Some(call) = self.call_at(i) else {
                i += 1;
                continue;
            };
            if is_control_or_declaration_call(&call.name) || is_enum_variant_call(&call.name) {
                i = call.close + 1;
                continue;
            }

            let args = split_top_level_args(self.tokens, call.open + 1, call.close);
            self.check_named_arguments(&call, &args);
            self.check_call_site_effects(&call, &args);
            self.check_retaining_local_values(&call, &args);
            i = call.close + 1;
        }
    }

    fn check_named_arguments(&mut self, call: &CallSite, args: &[ArgRange]) {
        for arg in args {
            if arg.start >= arg.end {
                continue;
            }
            if !is_named_arg(self.tokens, arg) {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0201",
                        format!("call to `{}` uses an unnamed argument.", call.name),
                        self.tokens[arg.start].span.clone(),
                        "argument must be named",
                    )
                    .with_cause(
                        "RSScript v0.4.1 requires all non-receiver call arguments to be named.",
                    )
                    .with_fix(
                        "add_argument_name",
                        "Write the argument as `name: value`.",
                        "manual",
                    ),
                );
            }
        }
    }

    fn check_call_site_effects(&mut self, call: &CallSite, args: &[ArgRange]) {
        let Some(function) = self.program.functions.get(&call.name) else {
            return;
        };
        let param_effects: HashMap<&str, &str> = function
            .params
            .iter()
            .filter_map(|param| {
                param
                    .effect
                    .as_deref()
                    .map(|effect| (param.name.as_str(), effect))
            })
            .collect();

        for arg in args {
            let Some((name, value_start, value_end)) = named_arg_parts(self.tokens, arg) else {
                continue;
            };
            let Some(expected) = param_effects.get(name) else {
                continue;
            };
            if value_start >= value_end || !self.tokens[value_start].is_ident_text(expected) {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0202",
                        format!("argument `{name}` for `{}` is missing `{expected}`.", call.name),
                        self.tokens[value_start.min(value_end.saturating_sub(1))].span.clone(),
                        "missing data effect",
                    )
                    .with_cause("Non-Copy parameters require an explicit `read`, `mut`, or `take` call-site effect.")
                    .with_fix(
                        "add_data_effect",
                        format!("Write `{name}: {expected} ...` at the call site."),
                        "machine-applicable",
                    ),
                );
            }
        }
    }

    fn check_retaining_local_values(&mut self, call: &CallSite, args: &[ArgRange]) {
        let Some(function) = self.program.functions.get(&call.name) else {
            return;
        };
        if function.retained_params.is_empty() {
            return;
        }
        let Some(owner) = self.enclosing_function(call.open) else {
            return;
        };
        let locals = collect_local_bindings(self.tokens, owner.body_start, owner.body_end);

        for arg in args {
            let Some((name, value_start, value_end)) = named_arg_parts(self.tokens, arg) else {
                continue;
            };
            if !function.retained_params.contains(name) {
                continue;
            }
            if value_start + 1 < value_end
                && self.tokens[value_start].is_ident_text("read")
                && ident_name(&self.tokens[value_start + 1]).is_some_and(|var| locals.contains(var))
            {
                let var = self.tokens[value_start + 1].text();
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0501",
                        format!(
                            "retaining API `{}` cannot retain local value `{var}`.",
                            call.name
                        ),
                        self.tokens[value_start + 1].span.clone(),
                        "local value retained",
                    )
                    .with_cause(format!(
                        "`{}` declares `effects(retains({name}))`.",
                        call.name
                    ))
                    .with_fix(
                        "manage_local",
                        format!(
                            "Pass `{name}: read (manage {var})` if the value should become managed."
                        ),
                        "manual",
                    ),
                );
            }
        }
    }

    fn check_functions(&mut self) {
        let functions: Vec<FunctionDecl> = self.program.functions.values().cloned().collect();
        for function in functions {
            self.check_function_body(&function);
            if function.returns_fresh {
                self.check_fresh_returns(&function);
            }
        }
    }

    fn check_function_body(&mut self, function: &FunctionDecl) {
        let mut locals: HashSet<String> = HashSet::new();
        let mut managed: HashSet<String> = HashSet::new();
        let mut moved: HashMap<String, Token> = HashMap::new();
        let mut i = function.body_start;

        while i < function.body_end {
            if self.tokens[i].is_ident_text("local") {
                if let Some(name) = self.tokens.get(i + 1).and_then(ident_name) {
                    locals.insert(name.to_string());
                    if self
                        .tokens
                        .get(i + 2)
                        .is_some_and(|token| token.symbol("="))
                        && self
                            .tokens
                            .get(i + 3)
                            .and_then(ident_name)
                            .is_some_and(|rhs| managed.contains(rhs))
                    {
                        self.diagnostics.push(
                            Diagnostic::error(
                                "RS0301",
                                format!(
                                    "managed value cannot be converted to local binding `{name}`."
                                ),
                                self.tokens[i + 3].span.clone(),
                                "managed value used as local",
                            )
                            .with_cause("RSScript has no managed -> local conversion.")
                            .with_fix(
                                "create_local",
                                "Create the value as `local` at its creation point.",
                                "manual",
                            ),
                        );
                    }
                }
            } else if self.tokens[i].is_ident_text("let") {
                if let Some(name) = self.tokens.get(i + 1).and_then(ident_name) {
                    managed.insert(name.to_string());
                    if self
                        .tokens
                        .get(i + 2)
                        .is_some_and(|token| token.symbol("="))
                        && self
                            .tokens
                            .get(i + 3)
                            .is_some_and(|token| token.symbol("|"))
                    {
                        self.check_managed_closure_capture(function, i, &locals);
                    }
                }
            } else if self.tokens[i].is_ident_text("manage") {
                if let Some(name) = self.tokens.get(i + 1).and_then(ident_name)
                    && locals.contains(name)
                {
                    moved.insert(name.to_string(), self.tokens[i].clone());
                }
            } else if self.tokens[i].is_ident_text("take") {
                self.check_take_of_handle_field(i);
            } else if self.tokens[i].is_ident_text("with") {
                if let Some(next) = self.check_resource_escape(i, function.body_end) {
                    i = next;
                    continue;
                }
            } else if let Some(name) = ident_name(&self.tokens[i])
                && let Some(move_token) = moved.get(name)
                && !previous_token_is(self.tokens, i, "manage")
                && !self
                    .tokens
                    .get(i + 1)
                    .is_some_and(|token| token.symbol(":"))
            {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0401",
                        format!("`{name}` was moved into the managed runtime by `manage {name}`."),
                        self.tokens[i].span.clone(),
                        "used after manage",
                    )
                    .with_cause(format!(
                        "The move happened at {}:{}.",
                        move_token.span.line, move_token.span.column
                    ))
                    .with_fix(
                        "move_use_before_manage",
                        format!("Move this use before `manage {name}`."),
                        "manual",
                    ),
                );
            }

            i += 1;
        }
    }

    fn check_managed_closure_capture(
        &mut self,
        function: &FunctionDecl,
        let_index: usize,
        locals: &HashSet<String>,
    ) {
        let Some(open) =
            (let_index..function.body_end).find(|index| self.tokens[*index].symbol("{"))
        else {
            return;
        };
        let Some(close) = find_matching(self.tokens, open, "{", "}") else {
            return;
        };
        for index in open + 1..close {
            if let Some(name) = ident_name(&self.tokens[index])
                && locals.contains(name)
            {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0801",
                        format!("managed closure captures local value `{name}`."),
                        self.tokens[index].span.clone(),
                        "local captured here",
                    )
                    .with_cause("Closures bound with `let` are managed closures.")
                    .with_fix(
                        "use_local_closure",
                        "Bind the closure with `local` or use a noescape callback.",
                        "manual",
                    ),
                );
            }
        }
    }

    fn check_take_of_handle_field(&mut self, take_index: usize) {
        if !(self
            .tokens
            .get(take_index + 2)
            .is_some_and(|token| token.symbol("."))
            && self
                .tokens
                .get(take_index + 3)
                .and_then(ident_name)
                .is_some())
        {
            return;
        }
        let field = self.tokens[take_index + 3].text();
        let is_handle = self.program.types.values().any(|decl| {
            decl.fields
                .iter()
                .any(|decl_field| decl_field.name == field && decl_field.is_handle)
        });
        if is_handle {
            self.diagnostics.push(
                Diagnostic::error(
                    "RS0901",
                    format!("cannot `take` handle field `{field}`."),
                    self.tokens[take_index].span.clone(),
                    "take of handle field",
                )
                .with_cause("Handle fields are managed references and cannot be consumed as local inline values."),
            );
        }
    }

    fn check_resource_escape(&mut self, with_index: usize, body_limit: usize) -> Option<usize> {
        let as_index =
            (with_index..body_limit).find(|index| self.tokens[*index].is_ident_text("as"))?;
        let resource_name = self
            .tokens
            .get(as_index + 1)
            .and_then(ident_name)?
            .to_string();
        let open = (as_index + 1..body_limit).find(|index| self.tokens[*index].symbol("{"))?;
        let close = find_matching(self.tokens, open, "{", "}")?;
        for index in open + 1..close {
            let escaping = self.tokens[index].is_ident_text("return")
                && self
                    .tokens
                    .get(index + 1)
                    .and_then(ident_name)
                    .is_some_and(|name| name == resource_name)
                || self.tokens[index].is_ident_text("manage")
                    && self
                        .tokens
                        .get(index + 1)
                        .and_then(ident_name)
                        .is_some_and(|name| name == resource_name);
            if escaping {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0702",
                        format!("resource `{resource_name}` cannot escape its `with` block."),
                        self.tokens[index].span.clone(),
                        "resource escapes",
                    )
                    .with_cause("A `with` resource must be dropped when the block exits."),
                );
            }
        }
        Some(close + 1)
    }

    fn check_fresh_returns(&mut self, function: &FunctionDecl) {
        let managed = collect_managed_bindings(self.tokens, function.body_start, function.body_end);
        let locals = collect_local_bindings(self.tokens, function.body_start, function.body_end);

        let mut i = function.body_start;
        while i < function.body_end {
            if !self.tokens[i].is_ident_text("return") {
                i += 1;
                continue;
            }
            let value_index = i + 1;
            if value_index >= function.body_end {
                i += 1;
                continue;
            }
            if let Some(name) = ident_name(&self.tokens[value_index]) {
                if managed.contains(name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0601",
                            format!(
                                "fresh function `{}` returns managed value `{name}`.",
                                function.name
                            ),
                            self.tokens[value_index].span.clone(),
                            "aliased value returned",
                        )
                        .with_cause(
                            "A `fresh` return must be newly created or a clean local value.",
                        )
                        .with_fix(
                            "return_fresh_value",
                            "Return a struct constructor, fresh call, or clean local binding.",
                            "manual",
                        ),
                    );
                } else if !locals.contains(name)
                    && !self
                        .program
                        .types
                        .get(name)
                        .is_some_and(|decl| decl.kind == TypeKind::Struct)
                    && !self
                        .program
                        .functions
                        .get(name)
                        .is_some_and(|decl| decl.returns_fresh)
                {
                    self.diagnostics.push(
                        Diagnostic::warning(
                            "RS0602",
                            format!("freshness of return value in `{}` could not be proven.", function.name),
                            self.tokens[value_index].span.clone(),
                            "freshness unknown",
                        )
                        .with_cause("This MVP checker only trusts clean locals, struct constructors, and known fresh functions."),
                    );
                }
            }
            i += 1;
        }
    }

    fn check_operator_overload_attempts(&mut self) {
        for i in 1..self.tokens.len().saturating_sub(1) {
            if !(self.tokens[i].symbol("+")
                || self.tokens[i].symbol("-")
                || self.tokens[i].symbol("*")
                || self.tokens[i].symbol("/"))
            {
                continue;
            }
            let left_number = matches!(self.tokens[i - 1].kind, TokenKind::Number(_));
            let right_number = matches!(self.tokens[i + 1].kind, TokenKind::Number(_));
            let likely_type_name = self.tokens[i - 1]
                .text()
                .chars()
                .next()
                .is_some_and(char::is_uppercase)
                || self.tokens[i + 1]
                    .text()
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase);
            if !left_number && !right_number && likely_type_name {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS1001",
                        "operators cannot be overloaded for user-defined types.",
                        self.tokens[i].span.clone(),
                        "operator on non-builtin-looking value",
                    )
                    .with_fix(
                        "use_named_function",
                        "Use a named function such as `Type.add(left: read a, right: read b)`.",
                        "manual",
                    ),
                );
            }
        }
    }

    fn call_at(&self, index: usize) -> Option<CallSite> {
        let direct_name = ident_name(self.tokens.get(index)?)?;
        if self
            .tokens
            .get(index + 1)
            .is_some_and(|token| token.symbol("("))
        {
            let open = index + 1;
            let close = find_matching(self.tokens, open, "(", ")")?;
            return Some(CallSite {
                name: direct_name.to_string(),
                open,
                close,
            });
        }

        if self
            .tokens
            .get(index + 1)
            .is_some_and(|token| token.symbol("."))
            && self.tokens.get(index + 2).and_then(ident_name).is_some()
            && self
                .tokens
                .get(index + 3)
                .is_some_and(|token| token.symbol("("))
        {
            let open = index + 3;
            let close = find_matching(self.tokens, open, "(", ")")?;
            return Some(CallSite {
                name: self.tokens[index + 2].text(),
                open,
                close,
            });
        }

        None
    }

    fn enclosing_function(&self, token_index: usize) -> Option<&FunctionDecl> {
        self.program
            .functions
            .values()
            .find(|function| token_index >= function.body_start && token_index < function.body_end)
    }
}

#[derive(Debug)]
struct CallSite {
    name: String,
    open: usize,
    close: usize,
}

#[derive(Debug)]
struct ArgRange {
    start: usize,
    end: usize,
}

fn split_top_level_args(tokens: &[Token], start: usize, end: usize) -> Vec<ArgRange> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut arg_start = start;
    for index in start..end {
        if tokens[index].symbol("(")
            || tokens[index].symbol("<")
            || tokens[index].symbol("[")
            || tokens[index].symbol("{")
        {
            depth += 1;
        } else if tokens[index].symbol(")")
            || tokens[index].symbol(">")
            || tokens[index].symbol("]")
            || tokens[index].symbol("}")
        {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && tokens[index].symbol(",") {
            args.push(ArgRange {
                start: arg_start,
                end: index,
            });
            arg_start = index + 1;
        }
    }
    if arg_start < end {
        args.push(ArgRange {
            start: arg_start,
            end,
        });
    }
    args
}

fn is_named_arg(tokens: &[Token], arg: &ArgRange) -> bool {
    tokens.get(arg.start).and_then(ident_name).is_some_and(|_| {
        tokens
            .get(arg.start + 1)
            .is_some_and(|token| token.symbol(":"))
    })
}

fn named_arg_parts<'a>(tokens: &'a [Token], arg: &ArgRange) -> Option<(&'a str, usize, usize)> {
    let name = tokens.get(arg.start).and_then(ident_name)?;
    if !tokens
        .get(arg.start + 1)
        .is_some_and(|token| token.symbol(":"))
    {
        return None;
    }
    Some((name, arg.start + 2, arg.end))
}

fn is_control_or_declaration_call(name: &str) -> bool {
    matches!(
        name,
        "fn" | "effects" | "retains" | "if" | "for" | "while" | "match" | "return"
    )
}

fn is_enum_variant_call(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some" | "None" | "Result" | "Option")
}

fn previous_token_is(tokens: &[Token], index: usize, text: &str) -> bool {
    index > 0 && tokens[index - 1].is_ident_text(text)
}

fn collect_local_bindings(tokens: &[Token], start: usize, end: usize) -> HashSet<String> {
    collect_bindings(tokens, start, end, "local")
}

fn collect_managed_bindings(tokens: &[Token], start: usize, end: usize) -> HashSet<String> {
    collect_bindings(tokens, start, end, "let")
}

fn collect_bindings(tokens: &[Token], start: usize, end: usize, keyword: &str) -> HashSet<String> {
    let mut bindings = HashSet::new();
    for index in start..end.saturating_sub(1) {
        if tokens[index].is_ident_text(keyword)
            && let Some(name) = tokens.get(index + 1).and_then(ident_name)
        {
            bindings.insert(name.to_string());
        }
    }
    bindings
}
