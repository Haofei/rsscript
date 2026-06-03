use std::collections::{BTreeMap, HashMap};

use crate::analyze_source_with_interfaces;
use crate::diagnostic::Diagnostic;
use crate::interfaces::standard_package_interfaces;
use crate::runtime_abi;
use crate::runtime_abi::InterpreterIntrinsic;
use crate::syntax::ast::{
    BinaryOp, Block, Callee, Expr, FunctionDecl, Item, MatchLiteral, MatchPattern, Program, Stmt,
};
use crate::syntax::parse_source;

include!(concat!(
    env!("OUT_DIR"),
    "/rss-interpreter-intrinsics-dispatch.rs"
));

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalOutput {
    pub value: String,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    Diagnostics(Vec<Diagnostic>),
    Runtime(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Unit,
    Int(i64),
    Bool(bool),
    String(String),
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
    Variant {
        name: String,
        fields: BTreeMap<String, Value>,
    },
}

impl Value {
    fn display(&self) -> String {
        match self {
            Self::Unit => "Unit".to_string(),
            Self::Int(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::Struct { name, fields } | Self::Variant { name, fields } => {
                if fields.is_empty() {
                    return name.clone();
                }
                let fields = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name} {{ {fields} }}")
            }
        }
    }
}

enum Control {
    Continue,
    Return(Value),
    Break,
    LoopContinue,
}

pub fn eval_source_main(file: &str, source: &str) -> Result<EvalOutput, EvalError> {
    let interfaces = standard_package_interfaces().collect::<Vec<_>>();
    let diagnostics = analyze_source_with_interfaces(file, source, &interfaces);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    let program = parse_source(file, source);
    let mut interpreter = Interpreter::new(&program);
    let value = interpreter.call_function("main", Vec::new())?;
    Ok(EvalOutput {
        value: value.display(),
        stdout: interpreter.stdout,
        stderr: interpreter.stderr,
    })
}

struct Interpreter<'a> {
    program: &'a Program,
    functions: HashMap<String, &'a FunctionDecl>,
    scopes: Vec<HashMap<String, Value>>,
    stdout: String,
    stderr: String,
}

impl<'a> Interpreter<'a> {
    fn new(program: &'a Program) -> Self {
        let functions = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(function) if function.has_body => {
                    Some((function.name.clone(), function))
                }
                _ => None,
            })
            .collect();
        Self {
            program,
            functions,
            scopes: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, EvalError> {
        let Some(function) = self.functions.get(name).copied() else {
            return Err(EvalError::Runtime(format!(
                "interpreter cannot call unsupported function `{name}`."
            )));
        };
        if function.is_async || function.is_native {
            return Err(EvalError::Runtime(format!(
                "interpreter P0 does not execute async/native function `{name}`."
            )));
        }
        if args.len() != function.params.len() {
            return Err(EvalError::Runtime(format!(
                "function `{name}` expected {} arguments, got {}.",
                function.params.len(),
                args.len()
            )));
        }
        self.scopes.push(HashMap::new());
        for (param, value) in function.params.iter().zip(args) {
            self.bind(param.name.clone(), value);
        }
        let result = match self.eval_block(&function.body)? {
            Control::Return(value) => value,
            Control::Continue => Value::Unit,
            Control::Break | Control::LoopContinue => {
                self.scopes.pop();
                return Err(EvalError::Runtime(format!(
                    "function `{name}` ended with a loop control statement."
                )));
            }
        };
        self.scopes.pop();
        Ok(result)
    }

    fn eval_block(&mut self, block: &Block) -> Result<Control, EvalError> {
        self.scopes.push(HashMap::new());
        for statement in &block.statements {
            match self.eval_stmt(statement)? {
                Control::Continue => {}
                control => {
                    self.scopes.pop();
                    return Ok(control);
                }
            }
        }
        self.scopes.pop();
        Ok(Control::Continue)
    }

    fn eval_stmt(&mut self, statement: &Stmt) -> Result<Control, EvalError> {
        match statement {
            Stmt::Let(stmt) => {
                let value = if let Some(value) = &stmt.value {
                    self.eval_expr(value)?
                } else {
                    Value::Unit
                };
                self.bind(stmt.name.clone(), value);
                Ok(Control::Continue)
            }
            Stmt::Assign(stmt) => {
                let value = self.eval_expr(&stmt.value)?;
                match &stmt.target {
                    Expr::Ident(name, _) => {
                        self.assign(name, value)?;
                        Ok(Control::Continue)
                    }
                    _ => Err(EvalError::Runtime(
                        "interpreter P0 only supports assignment to local variables.".to_string(),
                    )),
                }
            }
            Stmt::Return(stmt) => {
                let value = stmt
                    .value
                    .as_ref()
                    .map(|value| self.eval_expr(value))
                    .transpose()?
                    .unwrap_or(Value::Unit);
                Ok(Control::Return(value))
            }
            Stmt::If(stmt) => {
                if expect_bool(self.eval_expr(&stmt.condition)?)? {
                    self.eval_block(&stmt.then_body)
                } else if let Some(else_body) = &stmt.else_body {
                    self.eval_block(else_body)
                } else {
                    Ok(Control::Continue)
                }
            }
            Stmt::Loop(stmt) => {
                loop {
                    if let Some(condition) = &stmt.condition
                        && !expect_bool(self.eval_expr(condition)?)?
                    {
                        break;
                    }
                    match self.eval_block(&stmt.body)? {
                        Control::Continue | Control::LoopContinue => {}
                        Control::Break => break,
                        control @ Control::Return(_) => return Ok(control),
                    }
                }
                Ok(Control::Continue)
            }
            Stmt::Match(stmt) => {
                let value = self.eval_expr(&stmt.value)?;
                self.eval_match(value, &stmt.arms)
            }
            Stmt::Break(_) => Ok(Control::Break),
            Stmt::Continue(_) => Ok(Control::LoopContinue),
            Stmt::Expr(expr) => {
                self.eval_expr(expr)?;
                Ok(Control::Continue)
            }
            unsupported => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support statement `{}`.",
                stmt_name(unsupported)
            ))),
        }
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, EvalError> {
        match expr {
            Expr::Ident(name, _) if name == "Unit" => Ok(Value::Unit),
            Expr::Ident(name, _) if name == "true" => Ok(Value::Bool(true)),
            Expr::Ident(name, _) if name == "false" => Ok(Value::Bool(false)),
            Expr::Ident(name, _) => self.lookup(name),
            Expr::Number(value, _) => value
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|error| EvalError::Runtime(format!("invalid integer `{value}`: {error}"))),
            Expr::String(value, _) | Expr::MultilineString(value, _) => {
                Ok(Value::String(value.clone()))
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let left = self.eval_expr(left)?;
                let right = self.eval_expr(right)?;
                eval_binary(*op, left, right)
            }
            Expr::Field { base, name, .. } => {
                let base = self.eval_expr(base)?;
                match base {
                    Value::Struct { fields, .. } | Value::Variant { fields, .. } => fields
                        .get(name)
                        .cloned()
                        .ok_or_else(|| EvalError::Runtime(format!("unknown field `{name}`."))),
                    _ => Err(EvalError::Runtime(format!(
                        "cannot read field `{name}` from scalar value."
                    ))),
                }
            }
            Expr::Call { callee, args, .. } => self.eval_call(callee, args),
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => self.eval_expr(value),
            Expr::Match { value, arms, .. } => {
                let value = self.eval_expr(value)?;
                match self.eval_match(value, arms)? {
                    Control::Return(value) => Ok(value),
                    Control::Continue => Ok(Value::Unit),
                    Control::Break | Control::LoopContinue => Err(EvalError::Runtime(
                        "loop control cannot escape a match expression.".to_string(),
                    )),
                }
            }
            unsupported => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support expression `{}`.",
                expr_name(unsupported)
            ))),
        }
    }

    fn eval_call(
        &mut self,
        callee: &Callee,
        args: &[crate::syntax::ast::CallArg],
    ) -> Result<Value, EvalError> {
        match callee {
            Callee::Name(name) => {
                if self.functions.contains_key(name) {
                    let values = args
                        .iter()
                        .map(|arg| self.eval_expr(&arg.value))
                        .collect::<Result<Vec<_>, _>>()?;
                    return self.call_function(name, values);
                }
                if let Some(value) = self.construct_value(name, args)? {
                    return Ok(value);
                }
                Err(EvalError::Runtime(format!(
                    "interpreter cannot resolve call `{name}`."
                )))
            }
            Callee::Qualified { namespace, name } => {
                if let Some(intrinsic) = runtime_abi::lookup_runtime_intrinsic(namespace, name) {
                    return self.eval_runtime_intrinsic(
                        namespace,
                        name,
                        intrinsic.interpreter,
                        args,
                    );
                }
                Err(EvalError::Runtime(format!(
                    "interpreter P0 does not support qualified call `{namespace}.{name}`."
                )))
            }
            Callee::ReceiverCall {
                receiver, method, ..
            } => {
                let receiver = self.eval_expr(receiver)?;
                self.eval_receiver_call(receiver, method, args)
            }
        }
    }

    fn eval_runtime_intrinsic(
        &mut self,
        namespace: &str,
        name: &str,
        intrinsic: Option<InterpreterIntrinsic>,
        args: &[crate::syntax::ast::CallArg],
    ) -> Result<Value, EvalError> {
        match intrinsic {
            Some(intrinsic) => eval_generated_runtime_intrinsic(self, intrinsic, args),
            None => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support runtime intrinsic `{namespace}.{name}`."
            ))),
        }
    }

    fn eval_receiver_call(
        &mut self,
        receiver: Value,
        method: &str,
        args: &[crate::syntax::ast::CallArg],
    ) -> Result<Value, EvalError> {
        match (receiver, method) {
            (Value::Int(value), "to_string") => Ok(Value::String(value.to_string())),
            (Value::String(value), "len") => Ok(Value::Int(value.chars().count() as i64)),
            (Value::String(value), "is_empty") => Ok(Value::Bool(value.is_empty())),
            (Value::String(value), "concat") => {
                let right = self.eval_named_or_positional_arg(args, "right", 0)?;
                Ok(Value::String(format!("{value}{}", expect_string(right)?)))
            }
            (_, method) => Err(EvalError::Runtime(format!(
                "interpreter P0 does not support receiver method `{method}`."
            ))),
        }
    }

    fn eval_first_arg(&mut self, args: &[crate::syntax::ast::CallArg]) -> Result<Value, EvalError> {
        self.eval_named_or_positional_arg(args, "value", 0)
    }

    fn eval_named_or_positional_arg(
        &mut self,
        args: &[crate::syntax::ast::CallArg],
        name: &str,
        index: usize,
    ) -> Result<Value, EvalError> {
        let arg = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some(name))
            .or_else(|| args.get(index))
            .ok_or_else(|| EvalError::Runtime(format!("missing argument `{name}`.")))?;
        self.eval_expr(&arg.value)
    }

    fn construct_value(
        &mut self,
        name: &str,
        args: &[crate::syntax::ast::CallArg],
    ) -> Result<Option<Value>, EvalError> {
        if matches!(name, "Some" | "Ok" | "Err") {
            let value = self.eval_first_arg(args)?;
            return Ok(Some(Value::Variant {
                name: name.to_string(),
                fields: BTreeMap::from([("value".to_string(), value)]),
            }));
        }
        if name == "None" {
            return Ok(Some(Value::Variant {
                name: "None".to_string(),
                fields: BTreeMap::new(),
            }));
        }
        for item in &self.program.items {
            match item {
                Item::Type(type_decl) if type_decl.name == name => {
                    return Ok(Some(Value::Struct {
                        name: name.to_string(),
                        fields: self.eval_constructor_fields(&type_decl.fields, args)?,
                    }));
                }
                Item::SumType(sum) => {
                    if let Some(variant) = sum.variants.iter().find(|variant| variant.name == name)
                    {
                        return Ok(Some(Value::Variant {
                            name: name.to_string(),
                            fields: self.eval_constructor_fields(&variant.fields, args)?,
                        }));
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }

    fn eval_constructor_fields(
        &mut self,
        fields: &[crate::syntax::ast::FieldDecl],
        args: &[crate::syntax::ast::CallArg],
    ) -> Result<BTreeMap<String, Value>, EvalError> {
        let mut values = BTreeMap::new();
        for (index, field) in fields.iter().enumerate() {
            let arg = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some(field.name.as_str()))
                .or_else(|| args.get(index))
                .ok_or_else(|| EvalError::Runtime(format!("missing field `{}`.", field.name)))?;
            values.insert(field.name.clone(), self.eval_expr(&arg.value)?);
        }
        Ok(values)
    }

    fn eval_match(
        &mut self,
        value: Value,
        arms: &[crate::syntax::ast::MatchArm],
    ) -> Result<Control, EvalError> {
        for arm in arms {
            let mut bindings = HashMap::new();
            if pattern_matches(&arm.pattern, &value, &mut bindings) {
                self.scopes.push(bindings);
                let guard_matches = arm
                    .guard
                    .as_ref()
                    .map(|guard| self.eval_expr(guard).and_then(expect_bool))
                    .transpose()?
                    .unwrap_or(true);
                if guard_matches {
                    let result = self.eval_block(&arm.body);
                    self.scopes.pop();
                    return result;
                }
                self.scopes.pop();
            }
        }
        Err(EvalError::Runtime(
            "match reached no arm; checker should have rejected this.".to_string(),
        ))
    }

    fn bind(&mut self, name: String, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn assign(&mut self, name: &str, value: Value) -> Result<(), EvalError> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        Err(EvalError::Runtime(format!("unknown local `{name}`.")))
    }

    fn lookup(&self, name: &str) -> Result<Value, EvalError> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Ok(value.clone());
            }
        }
        Err(EvalError::Runtime(format!("unknown local `{name}`.")))
    }
}

fn pattern_matches(
    pattern: &MatchPattern,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> bool {
    match pattern {
        MatchPattern::Binding { name, .. } => {
            bindings.insert(name.clone(), value.clone());
            true
        }
        MatchPattern::Wildcard(_) => true,
        MatchPattern::Literal { value: literal, .. } => match (literal, value) {
            (MatchLiteral::Int(expected), Value::Int(actual)) => expected
                .parse::<i64>()
                .is_ok_and(|expected| expected == *actual),
            (MatchLiteral::String(expected), Value::String(actual)) => expected == actual,
            (MatchLiteral::Bool(expected), Value::Bool(actual)) => expected == actual,
            _ => false,
        },
        MatchPattern::Variant { name, binding, .. } => {
            let Value::Variant {
                name: actual_name,
                fields,
            } = value
            else {
                return false;
            };
            if name != actual_name {
                return false;
            }
            if let Some(binding) = binding {
                fields
                    .values()
                    .next()
                    .is_some_and(|value| pattern_matches(binding, value, bindings))
            } else {
                true
            }
        }
        MatchPattern::Struct { name, fields, .. } => {
            let (actual_name, actual_fields) = match value {
                Value::Struct { name, fields } | Value::Variant { name, fields } => (name, fields),
                _ => return false,
            };
            if name != actual_name {
                return false;
            }
            for field in fields {
                let Some(value) = actual_fields.get(&field.name) else {
                    return false;
                };
                if field.ignored {
                    continue;
                }
                if let Some(pattern) = &field.pattern {
                    if !pattern_matches(pattern, value, bindings) {
                        return false;
                    }
                } else if let Some(binding) = &field.binding {
                    bindings.insert(binding.clone(), value.clone());
                }
            }
            true
        }
    }
}

fn eval_binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, EvalError> {
    match op {
        BinaryOp::Add => match (left, right) {
            (Value::Int(left), Value::Int(right)) => Ok(Value::Int(left + right)),
            (Value::String(left), Value::String(right)) => {
                Ok(Value::String(format!("{left}{right}")))
            }
            _ => Err(EvalError::Runtime(
                "operator `+` expects matching Int or String operands.".to_string(),
            )),
        },
        BinaryOp::Subtract => Ok(Value::Int(expect_int(left)? - expect_int(right)?)),
        BinaryOp::Multiply => Ok(Value::Int(expect_int(left)? * expect_int(right)?)),
        BinaryOp::Divide => Ok(Value::Int(expect_int(left)? / expect_int(right)?)),
        BinaryOp::Equal => Ok(Value::Bool(left == right)),
        BinaryOp::NotEqual => Ok(Value::Bool(left != right)),
        BinaryOp::Less => Ok(Value::Bool(expect_int(left)? < expect_int(right)?)),
        BinaryOp::LessEqual => Ok(Value::Bool(expect_int(left)? <= expect_int(right)?)),
        BinaryOp::Greater => Ok(Value::Bool(expect_int(left)? > expect_int(right)?)),
        BinaryOp::GreaterEqual => Ok(Value::Bool(expect_int(left)? >= expect_int(right)?)),
        BinaryOp::LogicalAnd => Ok(Value::Bool(expect_bool(left)? && expect_bool(right)?)),
        BinaryOp::LogicalOr => Ok(Value::Bool(expect_bool(left)? || expect_bool(right)?)),
    }
}

fn expect_int(value: Value) -> Result<i64, EvalError> {
    match value {
        Value::Int(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Int, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_bool(value: Value) -> Result<bool, EvalError> {
    match value {
        Value::Bool(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected Bool, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_string(value: Value) -> Result<String, EvalError> {
    match value {
        Value::String(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "expected String, got `{}`.",
            other.display()
        ))),
    }
}

fn stmt_name(stmt: &Stmt) -> &'static str {
    match stmt {
        Stmt::With(_) | Stmt::MalformedWith(_) => "with",
        Stmt::For(_) | Stmt::MalformedFor(_) => "for",
        Stmt::TaskGroup(_) => "task_group",
        Stmt::Select(_) => "select",
        Stmt::LetElse(_) => "let-else",
        Stmt::MalformedIf(_) => "malformed-if",
        Stmt::MalformedLoop(_) => "malformed-loop",
        Stmt::MalformedMatch(_) => "malformed-match",
        Stmt::Unknown(_) => "unknown",
        _ => "statement",
    }
}

fn expr_name(expr: &Expr) -> &'static str {
    match expr {
        Expr::ObjectLiteral { .. } => "object-literal",
        Expr::MapLiteral { .. } => "map-literal",
        Expr::ArrayLiteral { .. } => "array-literal",
        Expr::Index { .. } => "index",
        Expr::Spawn { .. } => "spawn",
        Expr::Await { .. } => "await",
        Expr::Try { .. } => "try",
        Expr::Closure { .. } => "closure",
        Expr::Unknown(_) => "unknown",
        _ => "expression",
    }
}
