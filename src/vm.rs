use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::diagnostic::Severity;
use crate::hir::{Hir, HirBlock, HirCallArg, HirExpr, HirStmt};
use crate::interpreter::{CoverageBucket, EvalError, EvalOutput, NativeValue};
use crate::syntax::ast::{BinaryOp, Callee, MatchPattern};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VmCoverageReport {
    pub runtime_intrinsics: CoverageBucket,
    pub hir_statements: CoverageBucket,
    pub hir_expressions: CoverageBucket,
    pub value_types: CoverageBucket,
    pub function_kinds: CoverageBucket,
    pub parity_features: CoverageBucket,
}

pub fn vm_coverage_report() -> VmCoverageReport {
    let interpreter = crate::interpreter_coverage_report();

    VmCoverageReport {
        runtime_intrinsics: coverage_bucket(
            interpreter.runtime_intrinsics.supported,
            &[
                "Args.get_or_default",
                "List.get",
                "List.filter",
                "List.fold",
                "List.len",
                "List.map",
                "List.new",
                "List.pipeline",
                "List.push",
                "Log.write",
                "Map.get",
                "Map.insert",
                "Map.new",
                "Pipeline.collect",
                "Pipeline.filter",
                "Pipeline.map",
                "String.concat",
                "String.from_int",
                "String.parse_int",
            ],
        ),
        hir_statements: coverage_bucket(
            interpreter.hir_statements.supported,
            &["Assign", "Expr", "If", "Let", "Loop", "Match", "Return"],
        ),
        hir_expressions: coverage_bucket(
            interpreter.hir_expressions.supported,
            &[
                "Binary", "Call", "Closure", "Effect", "Field", "Ident", "Match", "Number",
                "String",
            ],
        ),
        value_types: coverage_bucket(
            interpreter.value_types.supported,
            &[
                "Bool", "Closure", "Int", "List", "Map", "String", "Struct", "Unit", "Variant",
            ],
        ),
        function_kinds: coverage_bucket(interpreter.function_kinds.supported, &["sync"]),
        parity_features: coverage_bucket(
            interpreter.parity_features.supported,
            &[
                "function:sync",
                "hir_expr:Binary",
                "hir_expr:Call",
                "hir_expr:Closure",
                "hir_expr:Effect",
                "hir_expr:Field",
                "hir_expr:Ident",
                "hir_expr:Match",
                "hir_expr:Number",
                "hir_expr:String",
                "hir_stmt:Assign",
                "hir_stmt:Expr",
                "hir_stmt:If",
                "hir_stmt:Let",
                "hir_stmt:Loop",
                "hir_stmt:Match",
                "hir_stmt:Return",
                "runtime:Args.get_or_default",
                "runtime:List.get",
                "runtime:List.filter",
                "runtime:List.fold",
                "runtime:List.len",
                "runtime:List.map",
                "runtime:List.new",
                "runtime:List.pipeline",
                "runtime:List.push",
                "runtime:Log.write",
                "runtime:Map.get",
                "runtime:Map.insert",
                "runtime:Map.new",
                "runtime:Pipeline.collect",
                "runtime:Pipeline.filter",
                "runtime:Pipeline.map",
                "runtime:String.concat",
                "runtime:String.from_int",
                "runtime:String.parse_int",
                "value:Bool",
                "value:Closure",
                "value:Int",
                "value:List",
                "value:Map",
                "value:String",
                "value:Struct",
                "value:Unit",
                "value:Variant",
            ],
        ),
    }
}

pub fn vm_eval_source_main_with_args(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    vm_compile_source(file, source)?.eval_main_with_args(args)
}

#[derive(Debug, Clone)]
pub struct VmExecutable {
    unit: VmUnit,
}

pub fn vm_compile_source(file: &str, source: &str) -> Result<VmExecutable, EvalError> {
    let diagnostics = crate::analyze_source_with_core(file, source);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    let program = parse_source(file, source);
    let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
    let unit = VmUnit::lower(&hir)?;
    Ok(VmExecutable { unit })
}

impl VmExecutable {
    pub fn eval_main_with_args(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        let mut vm = Vm::new(
            self.unit.clone(),
            args.into_iter().map(Into::into).collect(),
        );
        let value = vm.call_function("main", Vec::new())?;
        let display_value = value.display();
        let native_value = value.native_value();
        Ok(EvalOutput {
            value: display_value.clone(),
            display_value,
            native_value,
            stdout: vm.stdout,
            stderr: String::new(),
        })
    }
}

fn coverage_bucket(mut all: Vec<String>, supported: &[&str]) -> CoverageBucket {
    all.sort();
    all.dedup();
    let all_set = all
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut supported = supported
        .iter()
        .filter(|item| all_set.contains(**item))
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    supported.sort();
    let supported_set = supported
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let missing = all
        .iter()
        .filter(|item| !supported_set.contains(*item))
        .cloned()
        .collect();
    CoverageBucket {
        all,
        supported,
        missing,
    }
}

#[derive(Debug, Clone)]
struct VmUnit {
    functions: Vec<Rc<VmFunction>>,
    function_ids: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
struct VmFunction {
    name: String,
    locals: Vec<String>,
    local_ids: HashMap<String, usize>,
    params: usize,
    captures: usize,
    code: Vec<Instr>,
}

impl VmFunction {
    fn placeholder(name: String) -> Self {
        Self {
            name,
            locals: Vec::new(),
            local_ids: HashMap::new(),
            params: 0,
            captures: 0,
            code: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
enum Instr {
    Push(VmValue),
    LoadLocal(usize),
    StoreLocal(usize),
    Binary(BinaryOp),
    GetField(String),
    Jump(usize),
    JumpIfFalse(usize),
    CallUser {
        function: usize,
        argc: usize,
    },
    CallIntrinsic {
        intrinsic: VmIntrinsic,
        argc: usize,
    },
    MakeStruct {
        name: String,
        fields: Vec<String>,
    },
    MakeClosure {
        function: usize,
        captures: Vec<usize>,
    },
    MatchOption {
        some_ip: usize,
        none_ip: usize,
    },
    BindSome(usize),
    Return,
    Pop,
}

#[derive(Debug, Clone, Copy)]
enum VmIntrinsic {
    ArgsGetOrDefault,
    ListGet,
    ListLen,
    ListNew,
    ListPipeline,
    ListPush,
    ListFilter,
    ListFold,
    ListMap,
    LogWrite,
    MapGet,
    MapInsert,
    MapNew,
    OptionSome,
    PipelineCollect,
    PipelineFilter,
    PipelineMap,
    StringConcat,
    StringFromInt,
    StringParseInt,
}

#[derive(Debug, Clone)]
enum VmValue {
    Unit,
    Int(i64),
    Bool(bool),
    String(Rc<String>),
    List(Rc<RefCell<Vec<VmValue>>>),
    Map(Rc<RefCell<BTreeMap<VmMapKey, VmValue>>>),
    OptionSome(Box<VmValue>),
    OptionNone,
    Struct(Rc<VmStruct>),
    Closure(Rc<VmClosure>),
}

#[derive(Debug, Clone)]
struct VmStruct {
    name: Rc<str>,
    fields: HashMap<String, VmValue>,
}

#[derive(Debug, Clone)]
struct VmClosure {
    function: usize,
    captures: Vec<VmValue>,
}

enum VmArgs {
    Empty,
    One(VmValue),
    Two(VmValue, VmValue),
    Three(VmValue, VmValue, VmValue),
    Heap(Vec<VmValue>),
}

impl VmArgs {
    fn from_vec(values: Vec<VmValue>) -> Self {
        let mut values = values.into_iter();
        match values.len() {
            0 => Self::Empty,
            1 => Self::One(values.next().expect("one arg")),
            2 => Self::Two(
                values.next().expect("first arg"),
                values.next().expect("second arg"),
            ),
            3 => Self::Three(
                values.next().expect("first arg"),
                values.next().expect("second arg"),
                values.next().expect("third arg"),
            ),
            _ => Self::Heap(values.collect()),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::One(_) => 1,
            Self::Two(_, _) => 2,
            Self::Three(_, _, _) => 3,
            Self::Heap(values) => values.len(),
        }
    }

    fn into_vec(self) -> Vec<VmValue> {
        match self {
            Self::Empty => Vec::new(),
            Self::One(value) => vec![value],
            Self::Two(first, second) => vec![first, second],
            Self::Three(first, second, third) => vec![first, second, third],
            Self::Heap(values) => values,
        }
    }

    fn get(&self, index: usize) -> Option<&VmValue> {
        match (self, index) {
            (Self::One(value), 0) => Some(value),
            (Self::Two(value, _), 0) => Some(value),
            (Self::Two(_, value), 1) => Some(value),
            (Self::Three(value, _, _), 0) => Some(value),
            (Self::Three(_, value, _), 1) => Some(value),
            (Self::Three(_, _, value), 2) => Some(value),
            (Self::Heap(values), index) => values.get(index),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum VmMapKey {
    Bool(bool),
    Int(i64),
    String(String),
}

impl VmMapKey {
    fn from_value(value: VmValue) -> Result<Self, EvalError> {
        match value {
            VmValue::Bool(value) => Ok(Self::Bool(value)),
            VmValue::Int(value) => Ok(Self::Int(value)),
            VmValue::String(value) => Ok(Self::String(value.to_string())),
            other => Err(EvalError::Runtime(format!(
                "VM v0 Map key does not support `{}`.",
                other.display()
            ))),
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::String(value) => value.clone(),
        }
    }
}

impl VmValue {
    fn string(value: impl Into<String>) -> Self {
        Self::String(Rc::new(value.into()))
    }

    fn display(&self) -> String {
        match self {
            Self::Unit => "Unit".to_string(),
            Self::Int(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::String(value) => value.to_string(),
            Self::List(values) => {
                let values = values
                    .borrow()
                    .iter()
                    .map(VmValue::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
            Self::Map(entries) => {
                let values = entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.display(), value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{{{values}}}")
            }
            Self::OptionSome(value) => format!("Some({})", value.display()),
            Self::OptionNone => "None".to_string(),
            Self::Struct(data) => {
                let values = data
                    .fields
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.display()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}({values})", data.name)
            }
            Self::Closure(_) => "<closure>".to_string(),
        }
    }

    fn native_value(&self) -> Option<NativeValue> {
        match self {
            Self::Unit => Some(NativeValue::Unit),
            Self::Int(value) => Some(NativeValue::Int(*value)),
            Self::Bool(value) => Some(NativeValue::Bool(*value)),
            Self::String(value) => Some(NativeValue::String(value.to_string())),
            Self::List(_)
            | Self::Map(_)
            | Self::OptionSome(_)
            | Self::OptionNone
            | Self::Struct(_)
            | Self::Closure(_) => None,
        }
    }
}

impl PartialEq for VmValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) => true,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::OptionSome(left), Self::OptionSome(right)) => left == right,
            (Self::OptionNone, Self::OptionNone) => true,
            (Self::List(left), Self::List(right)) => *left.borrow() == *right.borrow(),
            (Self::Map(left), Self::Map(right)) => *left.borrow() == *right.borrow(),
            (Self::Struct(left), Self::Struct(right)) => {
                left.name == right.name && left.fields == right.fields
            }
            (Self::Closure(left), Self::Closure(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for VmValue {}

impl VmUnit {
    fn lower(hir: &Hir) -> Result<Self, EvalError> {
        let names = hir
            .function_bodies()
            .filter_map(|(name, body)| body.block.as_ref().map(|_| name.to_string()))
            .collect::<Vec<_>>();
        let function_ids = names
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), id))
            .collect::<HashMap<_, _>>();
        let mut functions = names
            .iter()
            .cloned()
            .map(VmFunction::placeholder)
            .collect::<Vec<_>>();
        for (function_id, name) in names.into_iter().enumerate() {
            let body = hir
                .function_body(&name)
                .and_then(|body| body.block.as_ref())
                .ok_or_else(|| EvalError::Runtime(format!("VM cannot find function `{name}`.")))?;
            let signature = hir.resolve_function(None, &name).ok_or_else(|| {
                EvalError::Runtime(format!("VM cannot resolve function `{name}`."))
            })?;
            let mut lowerer = FunctionLowerer {
                unit_function_ids: &function_ids,
                functions: &mut functions,
                function: VmFunction {
                    name,
                    locals: Vec::new(),
                    local_ids: HashMap::new(),
                    params: signature.params.len(),
                    captures: 0,
                    code: Vec::new(),
                },
            };
            for param in &signature.params {
                lowerer.local(&param.name);
            }
            lowerer.block(body)?;
            lowerer.emit(Instr::Push(VmValue::Unit));
            lowerer.emit(Instr::Return);
            functions[function_id] = lowerer.function;
        }
        Ok(Self {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids,
        })
    }
}

struct FunctionLowerer<'a> {
    unit_function_ids: &'a HashMap<String, usize>,
    functions: &'a mut Vec<VmFunction>,
    function: VmFunction,
}

impl FunctionLowerer<'_> {
    fn local(&mut self, name: &str) -> usize {
        if let Some(id) = self.function.local_ids.get(name) {
            return *id;
        }
        let id = self.function.locals.len();
        self.function.locals.push(name.to_string());
        self.function.local_ids.insert(name.to_string(), id);
        id
    }

    fn lookup_local(&self, name: &str) -> Result<usize, EvalError> {
        self.function
            .local_ids
            .get(name)
            .copied()
            .ok_or_else(|| EvalError::Runtime(format!("VM cannot resolve local `{name}`.")))
    }

    fn emit(&mut self, instr: Instr) -> usize {
        let ip = self.function.code.len();
        self.function.code.push(instr);
        ip
    }

    fn patch_jump(&mut self, jump_ip: usize, target: usize) {
        match &mut self.function.code[jump_ip] {
            Instr::Jump(ip) | Instr::JumpIfFalse(ip) => *ip = target,
            _ => {}
        }
    }

    fn block(&mut self, block: &HirBlock) -> Result<(), EvalError> {
        for statement in &block.statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    fn statement(&mut self, statement: &HirStmt) -> Result<(), EvalError> {
        match statement {
            HirStmt::Let { name, value, .. } => {
                if let Some(value) = value {
                    self.expr(value)?;
                } else {
                    self.emit(Instr::Push(VmValue::Unit));
                }
                let local = self.local(name);
                self.emit(Instr::StoreLocal(local));
            }
            HirStmt::Assign { target, value, .. } => {
                let HirExpr::Ident { name, .. } = target else {
                    return Err(EvalError::Runtime(
                        "VM v0 only supports assignment to local identifiers.".to_string(),
                    ));
                };
                self.expr(value)?;
                let local = self.lookup_local(name)?;
                self.emit(Instr::StoreLocal(local));
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.expr(value)?;
                } else {
                    self.emit(Instr::Push(VmValue::Unit));
                }
                self.emit(Instr::Return);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.expr(condition)?;
                let else_jump = self.emit(Instr::JumpIfFalse(usize::MAX));
                self.block(then_body)?;
                let end_jump = self.emit(Instr::Jump(usize::MAX));
                let else_ip = self.function.code.len();
                self.patch_jump(else_jump, else_ip);
                if let Some(else_body) = else_body {
                    self.block(else_body)?;
                }
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                let start_ip = self.function.code.len();
                let exit_jump = if let Some(condition) = condition {
                    self.expr(condition)?;
                    Some(self.emit(Instr::JumpIfFalse(usize::MAX)))
                } else {
                    None
                };
                self.block(body)?;
                self.emit(Instr::Jump(start_ip));
                if let Some(exit_jump) = exit_jump {
                    let exit_ip = self.function.code.len();
                    self.patch_jump(exit_jump, exit_ip);
                }
            }
            HirStmt::Match { value, arms, .. } => {
                self.option_match(value, arms, false)?;
            }
            HirStmt::Expr(expr) => {
                self.expr(expr)?;
                self.emit(Instr::Pop);
            }
            unsupported => {
                return Err(EvalError::Runtime(format!(
                    "VM v0 does not support statement `{unsupported:?}`."
                )));
            }
        }
        Ok(())
    }

    fn expr(&mut self, expr: &HirExpr) -> Result<(), EvalError> {
        match expr {
            HirExpr::Ident { name, .. } if name == "Unit" => {
                self.emit(Instr::Push(VmValue::Unit));
            }
            HirExpr::Ident { name, .. } => {
                let local = self.lookup_local(name)?;
                self.emit(Instr::LoadLocal(local));
            }
            HirExpr::Number { value, .. } => {
                let value = value.parse::<i64>().map_err(|error| {
                    EvalError::Runtime(format!("invalid VM integer `{value}`: {error}"))
                })?;
                self.emit(Instr::Push(VmValue::Int(value)));
            }
            HirExpr::String { value, .. } => {
                self.emit(Instr::Push(VmValue::string(decode_string_token(value))));
            }
            HirExpr::Binary {
                op, left, right, ..
            } => {
                self.expr(left)?;
                self.expr(right)?;
                self.emit(Instr::Binary(*op));
            }
            HirExpr::Field { base, name, .. } => {
                self.expr(base)?;
                self.emit(Instr::GetField(name.clone()));
            }
            HirExpr::Effect { value, .. } => self.expr(value)?,
            HirExpr::Call {
                callee,
                args,
                receiver,
                ..
            } => {
                if receiver.is_some() {
                    return Err(EvalError::Runtime(
                        "VM v0 does not support receiver calls.".to_string(),
                    ));
                }
                self.call(callee, args)?;
            }
            HirExpr::Match { value, arms, .. } => {
                self.option_match(value, arms, true)?;
            }
            HirExpr::Closure {
                params,
                captures,
                body,
                ..
            } => {
                let capture_locals = captures
                    .iter()
                    .map(|capture| self.lookup_local(&capture.name))
                    .collect::<Result<Vec<_>, _>>()?;
                let function_id = self.functions.len();
                self.functions
                    .push(VmFunction::placeholder(format!("<closure:{function_id}>")));
                let closure_function = {
                    let mut lowerer = FunctionLowerer {
                        unit_function_ids: self.unit_function_ids,
                        functions: &mut *self.functions,
                        function: VmFunction {
                            name: format!("<closure:{function_id}>"),
                            locals: Vec::new(),
                            local_ids: HashMap::new(),
                            params: params.len(),
                            captures: captures.len(),
                            code: Vec::new(),
                        },
                    };
                    for capture in captures {
                        lowerer.local(&capture.name);
                    }
                    for param in params {
                        lowerer.local(param);
                    }
                    lowerer.block(body)?;
                    lowerer.emit(Instr::Push(VmValue::Unit));
                    lowerer.emit(Instr::Return);
                    lowerer.function
                };
                self.functions[function_id] = closure_function;
                self.emit(Instr::MakeClosure {
                    function: function_id,
                    captures: capture_locals,
                });
            }
            unsupported => {
                return Err(EvalError::Runtime(format!(
                    "VM v0 does not support expression `{unsupported:?}`."
                )));
            }
        }
        Ok(())
    }

    fn option_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
        push_unit: bool,
    ) -> Result<(), EvalError> {
        self.expr(value)?;
        if arms.len() != 2 {
            return Err(EvalError::Runtime(
                "VM v0 only supports two-arm Option matches.".to_string(),
            ));
        }
        let match_ip = self.emit(Instr::MatchOption {
            some_ip: usize::MAX,
            none_ip: usize::MAX,
        });
        let mut end_jumps = Vec::new();
        let mut some_ip = None;
        let mut none_ip = None;
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant {
                    name,
                    binding: Some(binding),
                    ..
                } if name == "Some" => {
                    let ip = self.function.code.len();
                    some_ip = Some(ip);
                    for name in binding.binding_names() {
                        let local = self.local(name);
                        self.emit(Instr::BindSome(local));
                    }
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(Instr::Jump(usize::MAX)));
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    let ip = self.function.code.len();
                    none_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(Instr::Jump(usize::MAX)));
                }
                _ => {
                    return Err(EvalError::Runtime(
                        "VM v0 only supports Some/None match arms.".to_string(),
                    ));
                }
            }
        }
        let end_ip = self.function.code.len();
        if let Instr::MatchOption {
            some_ip: target_some_ip,
            none_ip: target_none_ip,
        } = &mut self.function.code[match_ip]
        {
            *target_some_ip = some_ip.ok_or_else(|| {
                EvalError::Runtime("VM Option match is missing Some arm.".to_string())
            })?;
            *target_none_ip = none_ip.ok_or_else(|| {
                EvalError::Runtime("VM Option match is missing None arm.".to_string())
            })?;
        }
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        if push_unit {
            self.emit(Instr::Push(VmValue::Unit));
        }
        Ok(())
    }

    fn call(&mut self, callee: &Callee, args: &[HirCallArg]) -> Result<(), EvalError> {
        match callee {
            Callee::Name(name) if name == "Some" => {
                self.lower_args(args)?;
                self.emit(Instr::CallIntrinsic {
                    intrinsic: VmIntrinsic::OptionSome,
                    argc: args.len(),
                });
                Ok(())
            }
            Callee::Name(name) => {
                if let Some(function) = self.unit_function_ids.get(name).copied() {
                    self.lower_args(args)?;
                    self.emit(Instr::CallUser {
                        function,
                        argc: args.len(),
                    });
                } else {
                    let fields = self.lower_named_args(args)?;
                    self.emit(Instr::MakeStruct {
                        name: type_root_name(name).to_string(),
                        fields,
                    });
                }
                Ok(())
            }
            Callee::Qualified { namespace, name } => {
                self.lower_args(args)?;
                let namespace_root = type_root_name(namespace);
                let name_root = type_root_name(name);
                let intrinsic = match (namespace_root, name_root) {
                    ("Args", "get_or_default") => VmIntrinsic::ArgsGetOrDefault,
                    ("List", "get") => VmIntrinsic::ListGet,
                    ("List", "filter") => VmIntrinsic::ListFilter,
                    ("List", "fold") => VmIntrinsic::ListFold,
                    ("List", "len") => VmIntrinsic::ListLen,
                    ("List", "map") => VmIntrinsic::ListMap,
                    ("List", "new") => VmIntrinsic::ListNew,
                    ("List", "pipeline") => VmIntrinsic::ListPipeline,
                    ("List", "push") => VmIntrinsic::ListPush,
                    ("Log", "write") => VmIntrinsic::LogWrite,
                    ("Map", "get") => VmIntrinsic::MapGet,
                    ("Map", "insert") => VmIntrinsic::MapInsert,
                    ("Map", "new") => VmIntrinsic::MapNew,
                    ("Pipeline", "collect") => VmIntrinsic::PipelineCollect,
                    ("Pipeline", "filter") => VmIntrinsic::PipelineFilter,
                    ("Pipeline", "map") => VmIntrinsic::PipelineMap,
                    ("String", "concat") => VmIntrinsic::StringConcat,
                    ("String", "from_int") => VmIntrinsic::StringFromInt,
                    ("String", "parse_int") => VmIntrinsic::StringParseInt,
                    _ => {
                        return Err(EvalError::Runtime(format!(
                            "VM v0 does not support intrinsic `{namespace}.{name}`."
                        )));
                    }
                };
                self.emit(Instr::CallIntrinsic {
                    intrinsic,
                    argc: args.len(),
                });
                Ok(())
            }
            Callee::ReceiverCall { .. } => Err(EvalError::Runtime(
                "VM v0 does not support receiver calls.".to_string(),
            )),
        }
    }

    fn lower_args(&mut self, args: &[HirCallArg]) -> Result<(), EvalError> {
        for arg in args {
            self.expr(&arg.value)?;
        }
        Ok(())
    }

    fn lower_named_args(&mut self, args: &[HirCallArg]) -> Result<Vec<String>, EvalError> {
        let mut fields = Vec::with_capacity(args.len());
        for arg in args {
            let Some(name) = &arg.name else {
                return Err(EvalError::Runtime(
                    "VM v0 struct constructors require named fields.".to_string(),
                ));
            };
            self.expr(&arg.value)?;
            fields.push(name.clone());
        }
        Ok(fields)
    }
}

struct Vm {
    unit: VmUnit,
    args: Vec<String>,
    stdout: String,
    stack: Vec<VmValue>,
    frames: Vec<VmFrame>,
}

struct VmFrame {
    function: Rc<VmFunction>,
    ip: usize,
    base: usize,
}

impl Vm {
    fn new(unit: VmUnit, args: Vec<String>) -> Self {
        Self {
            unit,
            args,
            stdout: String::new(),
            stack: Vec::new(),
            frames: Vec::new(),
        }
    }

    fn call_function(&mut self, name: &str, args: Vec<VmValue>) -> Result<VmValue, EvalError> {
        let function =
            self.unit.function_ids.get(name).copied().ok_or_else(|| {
                EvalError::Runtime(format!("VM cannot resolve function `{name}`."))
            })?;
        self.run_function(function, VmArgs::from_vec(args))
    }

    fn run_function(&mut self, function_id: usize, args: VmArgs) -> Result<VmValue, EvalError> {
        let entry_depth = self.frames.len();
        self.push_root_frame(function_id, args)?;
        self.run_until_depth(entry_depth)
    }

    fn run_until_depth(&mut self, entry_depth: usize) -> Result<VmValue, EvalError> {
        let mut cached_depth = usize::MAX;
        let mut cached_function: Option<Rc<VmFunction>> = None;

        while self.frames.len() > entry_depth {
            let frame_index = self.frames.len() - 1;
            if cached_depth != self.frames.len() {
                cached_depth = self.frames.len();
                cached_function = Some(Rc::clone(&self.frames[frame_index].function));
            }
            let ip = self.frames[frame_index].ip;
            let function = cached_function
                .as_ref()
                .expect("current function should be cached");
            let base = self.frames[frame_index].base;
            if ip >= function.code.len() {
                let frame = self.frames.pop().expect("frame should exist");
                cached_depth = usize::MAX;
                self.stack.truncate(frame.base);
                if self.frames.len() == entry_depth {
                    return Ok(VmValue::Unit);
                }
                self.stack.push(VmValue::Unit);
                continue;
            }
            self.frames[frame_index].ip += 1;

            match &function.code[ip] {
                Instr::Push(value) => self.stack.push(value.clone()),
                Instr::LoadLocal(local) => self.stack.push(self.stack[base + local].clone()),
                Instr::StoreLocal(local) => {
                    let value = self.pop()?;
                    self.stack[base + local] = value;
                }
                Instr::Binary(op) => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(eval_binary(*op, left, right)?);
                }
                Instr::GetField(name) => {
                    let value = self.pop()?;
                    self.stack.push(read_field(value, name)?);
                }
                Instr::Jump(target) => {
                    self.frames[frame_index].ip = *target;
                    continue;
                }
                Instr::JumpIfFalse(target) => {
                    if !expect_bool(self.pop()?)? {
                        self.frames[frame_index].ip = *target;
                        continue;
                    }
                }
                Instr::CallUser { function, argc } => {
                    self.push_call_frame(*function, *argc)?;
                    cached_depth = usize::MAX;
                    continue;
                }
                Instr::CallIntrinsic { intrinsic, argc } => {
                    let args = self.pop_args(*argc)?;
                    let value = self.call_intrinsic(*intrinsic, args)?;
                    self.stack.push(value);
                }
                Instr::MakeStruct { name, fields } => {
                    let values = self.pop_args(fields.len())?.into_vec();
                    let fields = fields
                        .iter()
                        .cloned()
                        .zip(values.into_iter())
                        .collect::<HashMap<_, _>>();
                    self.stack.push(VmValue::Struct(Rc::new(VmStruct {
                        name: Rc::from(name.as_str()),
                        fields,
                    })));
                }
                Instr::MakeClosure { function, captures } => {
                    let captures = captures
                        .iter()
                        .map(|local| self.stack[base + local].clone())
                        .collect::<Vec<_>>();
                    self.stack.push(VmValue::Closure(Rc::new(VmClosure {
                        function: *function,
                        captures,
                    })));
                }
                Instr::MatchOption { some_ip, none_ip } => match self.peek()? {
                    VmValue::OptionSome(_) => {
                        self.frames[frame_index].ip = *some_ip;
                        continue;
                    }
                    VmValue::OptionNone => {
                        let _ = self.pop()?;
                        self.frames[frame_index].ip = *none_ip;
                        continue;
                    }
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "VM Option match expected Option, got `{}`.",
                            other.display()
                        )));
                    }
                },
                Instr::BindSome(local) => {
                    let value = match self.pop()? {
                        VmValue::OptionSome(value) => *value,
                        other => {
                            return Err(EvalError::Runtime(format!(
                                "VM Some binding expected Some, got `{}`.",
                                other.display()
                            )));
                        }
                    };
                    self.stack[base + local] = value;
                }
                Instr::Return => {
                    let value = self.pop()?;
                    let frame = self.frames.pop().expect("frame should exist");
                    cached_depth = usize::MAX;
                    self.stack.truncate(frame.base);
                    if self.frames.len() == entry_depth {
                        return Ok(value);
                    }
                    self.stack.push(value);
                    continue;
                }
                Instr::Pop => {
                    let _ = self.pop()?;
                }
            }
        }
        Ok(VmValue::Unit)
    }

    fn push_root_frame(&mut self, function_id: usize, args: VmArgs) -> Result<(), EvalError> {
        let argc = args.len();
        let function = Rc::clone(&self.unit.functions[function_id]);
        self.push_args(args);
        self.push_frame(function, argc)
    }

    fn push_call_frame(&mut self, function_id: usize, argc: usize) -> Result<(), EvalError> {
        let function = Rc::clone(&self.unit.functions[function_id]);
        self.push_frame(function, argc)
    }

    fn push_closure_frame(
        &mut self,
        closure: Rc<VmClosure>,
        args: VmArgs,
    ) -> Result<(), EvalError> {
        let argc = closure.captures.len() + args.len();
        for capture in &closure.captures {
            self.stack.push(capture.clone());
        }
        self.push_args(args);
        let function = Rc::clone(&self.unit.functions[closure.function]);
        self.push_frame(function, argc)
    }

    fn push_frame(&mut self, function: Rc<VmFunction>, argc: usize) -> Result<(), EvalError> {
        let expected_args = function.captures + function.params;
        if argc != expected_args {
            return Err(EvalError::Runtime(format!(
                "VM function `{}` expected {} args, got {}.",
                function.name, expected_args, argc
            )));
        }
        if self.stack.len() < argc {
            return Err(EvalError::Runtime(
                "VM stack underflow for function call.".to_string(),
            ));
        }
        let base = self.stack.len() - argc;
        self.stack
            .resize(base + function.locals.len(), VmValue::Unit);
        self.frames.push(VmFrame {
            function,
            ip: 0,
            base,
        });
        Ok(())
    }

    fn push_args(&mut self, args: VmArgs) {
        match args {
            VmArgs::Empty => {}
            VmArgs::One(value) => self.stack.push(value),
            VmArgs::Two(first, second) => {
                self.stack.push(first);
                self.stack.push(second);
            }
            VmArgs::Three(first, second, third) => {
                self.stack.push(first);
                self.stack.push(second);
                self.stack.push(third);
            }
            VmArgs::Heap(values) => self.stack.extend(values),
        }
    }

    fn call_intrinsic(
        &mut self,
        intrinsic: VmIntrinsic,
        args: VmArgs,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            VmIntrinsic::ArgsGetOrDefault => {
                let index = expect_int(arg(&args, 0)?)?;
                let default = expect_string(arg(&args, 1)?)?;
                Ok(VmValue::string(
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| self.args.get(index).cloned())
                        .unwrap_or(default),
                ))
            }
            VmIntrinsic::ListGet => {
                let list = expect_list(arg(&args, 0)?)?;
                let index = expect_usize(arg(&args, 1)?)?;
                list.borrow().get(index).cloned().ok_or_else(|| {
                    EvalError::Runtime(format!("VM List.get index {index} out of bounds."))
                })
            }
            VmIntrinsic::ListFilter => {
                let list = expect_list(arg(&args, 0)?)?;
                let predicate = expect_closure(arg(&args, 1)?)?;
                self.filter_list(list, predicate)
            }
            VmIntrinsic::ListFold => {
                let list = expect_list(arg(&args, 0)?)?;
                let mut state = arg(&args, 1)?;
                let folder = expect_closure(arg(&args, 2)?)?;
                let items = list.borrow().clone();
                for item in items {
                    state = self.call_closure(folder.clone(), VmArgs::Two(state, item))?;
                }
                Ok(state)
            }
            VmIntrinsic::ListLen => {
                let list = expect_list(arg(&args, 0)?)?;
                Ok(VmValue::Int(list.borrow().len() as i64))
            }
            VmIntrinsic::ListMap => {
                let list = expect_list(arg(&args, 0)?)?;
                let mapper = expect_closure(arg(&args, 1)?)?;
                self.map_list(list, mapper)
            }
            VmIntrinsic::ListNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            VmIntrinsic::ListPipeline => Ok(arg(&args, 0)?),
            VmIntrinsic::ListPush => {
                let list = expect_list(arg(&args, 0)?)?;
                list.borrow_mut().push(arg(&args, 1)?);
                Ok(VmValue::Unit)
            }
            VmIntrinsic::LogWrite => {
                self.stdout.push_str(&expect_string(arg(&args, 0)?)?);
                self.stdout.push('\n');
                Ok(VmValue::Unit)
            }
            VmIntrinsic::MapGet => {
                let map = expect_map(arg(&args, 0)?)?;
                let key = VmMapKey::from_value(arg(&args, 1)?)?;
                Ok(map
                    .borrow()
                    .get(&key)
                    .cloned()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            VmIntrinsic::MapInsert => {
                let map = expect_map(arg(&args, 0)?)?;
                let key = VmMapKey::from_value(arg(&args, 1)?)?;
                let value = arg(&args, 2)?;
                map.borrow_mut().insert(key, value);
                Ok(VmValue::Unit)
            }
            VmIntrinsic::MapNew => Ok(VmValue::Map(Rc::new(RefCell::new(BTreeMap::new())))),
            VmIntrinsic::OptionSome => Ok(VmValue::OptionSome(Box::new(arg(&args, 0)?))),
            VmIntrinsic::StringConcat => {
                let left = expect_string(arg(&args, 0)?)?;
                let right = expect_string(arg(&args, 1)?)?;
                Ok(VmValue::string(format!("{left}{right}")))
            }
            VmIntrinsic::StringFromInt => {
                Ok(VmValue::string(expect_int(arg(&args, 0)?)?.to_string()))
            }
            VmIntrinsic::StringParseInt => {
                let value = expect_string(arg(&args, 0)?)?;
                Ok(match value.parse::<i64>() {
                    Ok(value) => VmValue::OptionSome(Box::new(VmValue::Int(value))),
                    Err(_) => VmValue::OptionNone,
                })
            }
            VmIntrinsic::PipelineCollect => Ok(arg(&args, 0)?),
            VmIntrinsic::PipelineFilter => {
                let list = expect_list(arg(&args, 0)?)?;
                let predicate = expect_closure(arg(&args, 1)?)?;
                self.filter_list(list, predicate)
            }
            VmIntrinsic::PipelineMap => {
                let list = expect_list(arg(&args, 0)?)?;
                let mapper = expect_closure(arg(&args, 1)?)?;
                self.map_list(list, mapper)
            }
        }
    }

    fn filter_list(
        &mut self,
        list: Rc<RefCell<Vec<VmValue>>>,
        predicate: Rc<VmClosure>,
    ) -> Result<VmValue, EvalError> {
        let items = list.borrow().clone();
        let mut filtered = Vec::new();
        for item in items {
            let keep = self.call_closure(predicate.clone(), VmArgs::One(item.clone()))?;
            if expect_bool(keep)? {
                filtered.push(item);
            }
        }
        Ok(VmValue::List(Rc::new(RefCell::new(filtered))))
    }

    fn map_list(
        &mut self,
        list: Rc<RefCell<Vec<VmValue>>>,
        mapper: Rc<VmClosure>,
    ) -> Result<VmValue, EvalError> {
        let items = list.borrow().clone();
        let mut mapped = Vec::with_capacity(items.len());
        for item in items {
            mapped.push(self.call_closure(mapper.clone(), VmArgs::One(item))?);
        }
        Ok(VmValue::List(Rc::new(RefCell::new(mapped))))
    }

    fn call_closure(&mut self, closure: Rc<VmClosure>, args: VmArgs) -> Result<VmValue, EvalError> {
        let entry_depth = self.frames.len();
        self.push_closure_frame(closure, args)?;
        self.run_until_depth(entry_depth)
    }

    fn pop(&mut self) -> Result<VmValue, EvalError> {
        self.stack
            .pop()
            .ok_or_else(|| EvalError::Runtime("VM stack underflow.".to_string()))
    }

    fn peek(&self) -> Result<&VmValue, EvalError> {
        self.stack
            .last()
            .ok_or_else(|| EvalError::Runtime("VM stack underflow.".to_string()))
    }

    fn pop_args(&mut self, argc: usize) -> Result<VmArgs, EvalError> {
        if self.stack.len() < argc {
            return Err(EvalError::Runtime(
                "VM stack underflow for call.".to_string(),
            ));
        }
        Ok(match argc {
            0 => VmArgs::Empty,
            1 => VmArgs::One(self.pop()?),
            2 => {
                let second = self.pop()?;
                let first = self.pop()?;
                VmArgs::Two(first, second)
            }
            3 => {
                let third = self.pop()?;
                let second = self.pop()?;
                let first = self.pop()?;
                VmArgs::Three(first, second, third)
            }
            _ => {
                let start = self.stack.len() - argc;
                VmArgs::Heap(self.stack.drain(start..).collect())
            }
        })
    }
}

fn arg(args: &VmArgs, index: usize) -> Result<VmValue, EvalError> {
    args.get(index)
        .cloned()
        .ok_or_else(|| EvalError::Runtime(format!("VM missing argument {index}.")))
}

fn expect_int(value: VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Int(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "VM expected Int, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_usize(value: VmValue) -> Result<usize, EvalError> {
    let value = expect_int(value)?;
    usize::try_from(value)
        .map_err(|_| EvalError::Runtime(format!("VM expected non-negative index, got `{value}`.")))
}

fn expect_bool(value: VmValue) -> Result<bool, EvalError> {
    match value {
        VmValue::Bool(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "VM expected Bool, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_list(value: VmValue) -> Result<Rc<RefCell<Vec<VmValue>>>, EvalError> {
    match value {
        VmValue::List(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "VM expected List, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_map(value: VmValue) -> Result<Rc<RefCell<BTreeMap<VmMapKey, VmValue>>>, EvalError> {
    match value {
        VmValue::Map(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "VM expected Map, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_closure(value: VmValue) -> Result<Rc<VmClosure>, EvalError> {
    match value {
        VmValue::Closure(value) => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "VM expected Closure, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_string(value: VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::String(value) => Ok(value.to_string()),
        other => Err(EvalError::Runtime(format!(
            "VM expected String, got `{}`.",
            other.display()
        ))),
    }
}

fn read_field(value: VmValue, field: &str) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(data) => data.fields.get(field).cloned().ok_or_else(|| {
            EvalError::Runtime(format!("VM struct value is missing field `{field}`."))
        }),
        other => Err(EvalError::Runtime(format!(
            "VM expected Struct for field `{field}`, got `{}`.",
            other.display()
        ))),
    }
}

fn eval_binary(op: BinaryOp, left: VmValue, right: VmValue) -> Result<VmValue, EvalError> {
    match op {
        BinaryOp::Add => Ok(VmValue::Int(expect_int(left)? + expect_int(right)?)),
        BinaryOp::Subtract => Ok(VmValue::Int(expect_int(left)? - expect_int(right)?)),
        BinaryOp::Multiply => Ok(VmValue::Int(expect_int(left)? * expect_int(right)?)),
        BinaryOp::Divide => Ok(VmValue::Int(expect_int(left)? / expect_int(right)?)),
        BinaryOp::Equal => Ok(VmValue::Bool(left == right)),
        BinaryOp::NotEqual => Ok(VmValue::Bool(left != right)),
        BinaryOp::Less => Ok(VmValue::Bool(expect_int(left)? < expect_int(right)?)),
        BinaryOp::LessEqual => Ok(VmValue::Bool(expect_int(left)? <= expect_int(right)?)),
        BinaryOp::Greater => Ok(VmValue::Bool(expect_int(left)? > expect_int(right)?)),
        BinaryOp::GreaterEqual => Ok(VmValue::Bool(expect_int(left)? >= expect_int(right)?)),
        BinaryOp::LogicalAnd => Ok(VmValue::Bool(expect_bool(left)? && expect_bool(right)?)),
        BinaryOp::LogicalOr => Ok(VmValue::Bool(expect_bool(left)? || expect_bool(right)?)),
    }
}

fn decode_string_token(value: &str) -> String {
    let mut decoded = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\\') => decoded.push('\\'),
            Some('"') => decoded.push('"'),
            Some('0') => decoded.push('\0'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }
    decoded
}

fn type_root_name(name: &str) -> &str {
    name.split_once('<')
        .map(|(root, _)| root.trim())
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_value_representation_stays_compact() {
        assert_eq!(std::mem::size_of::<VmValue>(), 16);
    }
}
