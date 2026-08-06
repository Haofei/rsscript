use std::collections::BTreeMap;

use rsscript_abi_model::{
    DataEffect as AbiDataEffect, ExternalSymbol, FunctionSignature, ParameterSignature,
};
use rsscript_semantics::hir as checked;
use rsscript_syntax::ast as syntax;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamEffect {
    Read,
    Mut,
    Take,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutableTypeKind {
    Class,
    Struct,
    Resource,
    Sum,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableParam {
    pub name: String,
    pub effect: Option<ParamEffect>,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableSignature {
    pub namespace: Option<String>,
    pub name: String,
    pub is_async: bool,
    pub params: Vec<ExecutableParam>,
    pub return_type: Option<String>,
    pub is_external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableFieldInfo {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableTypeInfo {
    pub name: String,
    pub kind: ExecutableTypeKind,
    pub fields_ordered: Vec<ExecutableFieldInfo>,
    pub fields: BTreeMap<String, ExecutableFieldInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableFunction {
    pub name: String,
    pub is_async: bool,
    pub signature: ExecutableSignature,
    pub body: ExecutableBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableBlock {
    pub statements: Vec<ExecutableStmt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableStmt {
    Let {
        name: String,
        value: Option<ExecutableExpr>,
        is_async: bool,
    },
    Return {
        value: Option<ExecutableExpr>,
    },
    With {
        resource: ExecutableExpr,
        binding: String,
        body: ExecutableBlock,
    },
    If {
        condition: ExecutableExpr,
        then_body: ExecutableBlock,
        else_body: Option<ExecutableBlock>,
    },
    Loop {
        condition: Option<ExecutableExpr>,
        body: ExecutableBlock,
    },
    For {
        binding: String,
        iterable: ExecutableExpr,
        is_async: bool,
        body: ExecutableBlock,
    },
    Match {
        value: ExecutableExpr,
        arms: Vec<ExecutableMatchArm>,
    },
    Select {
        arms: Vec<ExecutableSelectArm>,
    },
    Assign {
        target: ExecutableExpr,
        value: ExecutableExpr,
    },
    Break,
    Continue,
    Expr(ExecutableExpr),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableMatchArm {
    pub pattern: MatchPattern,
    pub guard: Option<ExecutableExpr>,
    pub body: ExecutableBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableSelectArm {
    pub binding: String,
    pub operation: ExecutableExpr,
    pub body: ExecutableBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableCallReceiver {
    pub value: Box<ExecutableExpr>,
    pub effect: ParamEffect,
    pub type_name: Option<String>,
    pub resolved_namespace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableFieldAccess {
    pub base_type: Option<String>,
    pub type_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableClosureCapture {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableObjectLiteralField {
    pub name: String,
    pub value: ExecutableExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableMapLiteralEntry {
    pub key: ExecutableExpr,
    pub value: ExecutableExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableCallArg {
    pub name: Option<String>,
    pub value: ExecutableExpr,
    pub parameter_index: Option<usize>,
    pub evaluation_index: usize,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutableExpr {
    Ident {
        name: String,
        type_name: Option<String>,
    },
    Number {
        value: String,
    },
    String {
        value: String,
    },
    Char {
        value: String,
    },
    ObjectLiteral {
        fields: Vec<ExecutableObjectLiteralField>,
        type_name: Option<String>,
    },
    MapLiteral {
        entries: Vec<ExecutableMapLiteralEntry>,
        type_name: Option<String>,
    },
    ArrayLiteral {
        items: Vec<ExecutableExpr>,
        type_name: Option<String>,
    },
    Binary {
        op: BinaryOp,
        left: Box<ExecutableExpr>,
        right: Box<ExecutableExpr>,
    },
    Field {
        base: Box<ExecutableExpr>,
        name: String,
        access: ExecutableFieldAccess,
    },
    Index {
        base: Box<ExecutableExpr>,
        index: Box<ExecutableExpr>,
    },
    Call {
        callee: Callee,
        receiver: Option<ExecutableCallReceiver>,
        args: Vec<ExecutableCallArg>,
        type_name: Option<String>,
    },
    Effect {
        effect: ParamEffect,
        value: Box<ExecutableExpr>,
        type_name: Option<String>,
    },
    Manage {
        value: Box<ExecutableExpr>,
        type_name: Option<String>,
    },
    Spawn {
        value: Box<ExecutableExpr>,
        type_name: Option<String>,
    },
    Await {
        value: Box<ExecutableExpr>,
        type_name: Option<String>,
    },
    Try {
        value: Box<ExecutableExpr>,
        type_name: Option<String>,
    },
    Closure {
        params: Vec<String>,
        captures: Vec<ExecutableClosureCapture>,
        explicit: bool,
        body: ExecutableBlock,
    },
    Match {
        value: Box<ExecutableExpr>,
        arms: Vec<ExecutableMatchArm>,
        type_name: Option<String>,
    },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    Name(String),
    Qualified { namespace: String, name: String },
    ReceiverCall { method: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchPattern {
    Binding {
        name: String,
    },
    Variant {
        name: String,
        bindings: Vec<MatchPattern>,
    },
    Struct {
        name: String,
        fields: Vec<MatchFieldPattern>,
        has_rest: bool,
    },
    Literal {
        value: MatchLiteral,
    },
    List {
        prefix: Vec<MatchPattern>,
        rest: Option<Option<String>>,
        suffix: Vec<MatchPattern>,
    },
    Wildcard,
}

impl MatchPattern {
    pub fn binding_names(&self) -> Vec<&str> {
        match self {
            Self::Binding { name } => vec![name],
            Self::Variant { bindings, .. } => {
                bindings.iter().flat_map(Self::binding_names).collect()
            }
            Self::Struct { fields, .. } => fields
                .iter()
                .flat_map(|field| {
                    if field.ignored {
                        Vec::new()
                    } else if let Some(pattern) = &field.pattern {
                        pattern.binding_names()
                    } else {
                        field.binding.as_deref().into_iter().collect()
                    }
                })
                .collect(),
            Self::List {
                prefix,
                rest,
                suffix,
            } => {
                let mut names = prefix
                    .iter()
                    .flat_map(Self::binding_names)
                    .collect::<Vec<_>>();
                if let Some(Some(rest)) = rest {
                    names.push(rest);
                }
                names.extend(suffix.iter().flat_map(Self::binding_names));
                names
            }
            Self::Literal { .. } | Self::Wildcard => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchFieldPattern {
    pub name: String,
    pub binding: Option<String>,
    pub pattern: Option<Box<MatchPattern>>,
    pub ignored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchLiteral {
    Int(String),
    String(String),
    Char(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableExternalImport {
    pub symbol: ExternalSymbol,
    pub signature: FunctionSignature,
}

#[derive(Debug, Clone, Default)]
pub struct ExecutableProgram {
    functions: BTreeMap<String, ExecutableFunction>,
    signatures: BTreeMap<String, ExecutableSignature>,
    types: BTreeMap<String, ExecutableTypeInfo>,
    resource_drop_bodies: BTreeMap<String, ExecutableBlock>,
    sum_variant_types: BTreeMap<String, String>,
    sum_variant_fields: BTreeMap<String, Vec<ExecutableFieldInfo>>,
    protocol_dispatch: BTreeMap<(String, String), Vec<(String, String)>>,
}

impl ExecutableProgram {
    pub fn functions(&self) -> impl Iterator<Item = &ExecutableFunction> {
        self.functions.values()
    }

    pub fn function(&self, name: &str) -> Option<&ExecutableFunction> {
        self.functions.get(name)
    }

    pub fn resolve_function(
        &self,
        namespace: Option<&str>,
        name: &str,
    ) -> Option<&ExecutableSignature> {
        if let Some(namespace) = namespace {
            let qualified = format!("{namespace}.{name}");
            if let Some(signature) = self.signatures.get(&qualified) {
                return Some(signature);
            }
            let qualified = format!("{}.{}", type_root_name(namespace), name);
            if let Some(signature) = self.signatures.get(&qualified) {
                return Some(signature);
            }
        }
        namespace
            .is_none()
            .then(|| self.signatures.get(name))
            .flatten()
    }

    pub fn type_info(&self, name: &str) -> Option<&ExecutableTypeInfo> {
        self.types.get(type_root_name(name))
    }

    pub fn type_kind(&self, name: &str) -> Option<ExecutableTypeKind> {
        self.type_info(name).map(|info| info.kind)
    }

    pub fn types(&self) -> impl Iterator<Item = &ExecutableTypeInfo> {
        self.types.values()
    }

    pub fn resource_drop_bodies(&self) -> impl Iterator<Item = (&str, &ExecutableBlock)> {
        self.resource_drop_bodies
            .iter()
            .map(|(name, body)| (name.as_str(), body))
    }

    pub fn sum_type_for_variant(&self, name: &str) -> Option<&str> {
        self.sum_variant_types.get(name).map(String::as_str)
    }

    pub fn sum_variant_fields(&self, name: &str) -> Option<&[ExecutableFieldInfo]> {
        self.sum_variant_fields.get(name).map(Vec::as_slice)
    }

    pub fn protocol_method_targets(&self, protocol: &str, method: &str) -> Vec<(String, String)> {
        self.protocol_dispatch
            .get(&(
                type_root_name(protocol).to_owned(),
                type_root_name(method).to_owned(),
            ))
            .cloned()
            .unwrap_or_default()
    }
}

pub(crate) fn project_hir(
    hir: &checked::Hir,
) -> (ExecutableProgram, Box<[ExecutableExternalImport]>) {
    let mut program = ExecutableProgram::default();
    for type_info in hir.types() {
        let projected = project_type(type_info);
        program.types.insert(projected.name.clone(), projected);
    }
    for (name, body) in hir.resource_drop_bodies() {
        program
            .resource_drop_bodies
            .insert(name.to_owned(), project_block(body));
    }
    for (name, body) in hir.function_bodies() {
        let Some(block) = &body.block else { continue };
        let Some(signature) = hir.resolve_function(None, name) else {
            continue;
        };
        let signature = project_signature(hir, signature);
        program.functions.insert(
            name.to_owned(),
            ExecutableFunction {
                name: name.to_owned(),
                is_async: signature.is_async,
                signature,
                body: project_block(block),
            },
        );
    }
    for (key, signature) in hir.signatures() {
        program
            .signatures
            .insert(key.to_owned(), project_signature(hir, signature));
    }
    for call in hir.call_sites() {
        if let syntax::Callee::Qualified { namespace, name } = &call.callee {
            let key = (
                type_root_name(namespace).to_owned(),
                type_root_name(name).to_owned(),
            );
            program
                .protocol_dispatch
                .entry(key)
                .or_insert_with(|| hir.protocol_method_targets(namespace, name));
        }
    }
    for (variant, owner, fields) in hir.sum_variants() {
        program
            .sum_variant_types
            .insert(variant.to_owned(), owner.to_owned());
        program.sum_variant_fields.insert(
            variant.to_owned(),
            fields.iter().map(project_field).collect(),
        );
    }

    let imports = hir
        .call_sites()
        .iter()
        .filter_map(|call| match &call.resolution {
            checked::CallResolution::Resolved { signature, .. } if signature.is_external => {
                let symbol = signature.namespace.as_ref().map_or_else(
                    || signature.name.clone(),
                    |namespace| format!("{namespace}.{}", signature.name),
                );
                Some(ExecutableExternalImport {
                    symbol: ExternalSymbol::new(symbol).ok()?,
                    signature: abi_signature(signature),
                })
            }
            _ => None,
        })
        .fold(BTreeMap::new(), |mut imports, import| {
            imports.entry(import.symbol.clone()).or_insert(import);
            imports
        });
    (
        program,
        imports.into_values().collect::<Vec<_>>().into_boxed_slice(),
    )
}

fn project_signature(hir: &checked::Hir, signature: &checked::FunctionSig) -> ExecutableSignature {
    ExecutableSignature {
        namespace: signature.namespace.clone(),
        name: signature.name.clone(),
        is_async: signature.is_async,
        params: signature
            .params
            .iter()
            .map(|param| ExecutableParam {
                name: param.name.clone(),
                effect: param.effect.map(project_param_effect),
                type_name: hir.canonical_type_name(&param.ty.to_string()),
            })
            .collect(),
        return_type: signature
            .return_ty
            .as_ref()
            .map(|ty| hir.canonical_type_name(&ty.to_string())),
        is_external: signature.is_external,
    }
}

fn abi_signature(signature: &checked::FunctionSig) -> FunctionSignature {
    FunctionSignature {
        parameters: signature
            .params
            .iter()
            .map(|parameter| ParameterSignature {
                name: parameter.name.clone(),
                effect: match parameter.effect.unwrap_or(checked::ParamEffect::Read) {
                    checked::ParamEffect::Read => AbiDataEffect::Read,
                    checked::ParamEffect::Mut => AbiDataEffect::Mut,
                    checked::ParamEffect::Take => AbiDataEffect::Take,
                },
                ty: parameter.ty.to_string().into(),
                retained: signature.retained_params.contains(&parameter.name),
            })
            .collect(),
        result: signature
            .return_ty
            .as_ref()
            .map_or_else(|| "Unit".into(), |ty| ty.to_string().into()),
        asynchronous: signature.is_async,
    }
}

fn project_type(info: &checked::TypeInfo) -> ExecutableTypeInfo {
    let fields_ordered = info
        .fields_ordered
        .iter()
        .map(project_field)
        .collect::<Vec<_>>();
    ExecutableTypeInfo {
        name: info.name.clone(),
        kind: match info.kind {
            checked::HirTypeKind::Class => ExecutableTypeKind::Class,
            checked::HirTypeKind::Struct => ExecutableTypeKind::Struct,
            checked::HirTypeKind::Resource => ExecutableTypeKind::Resource,
            checked::HirTypeKind::Sum => ExecutableTypeKind::Sum,
        },
        fields: fields_ordered
            .iter()
            .cloned()
            .map(|field| (field.name.clone(), field))
            .collect(),
        fields_ordered,
    }
}

fn type_root_name(name: &str) -> &str {
    name.split_once('<').map_or(name, |(root, _)| root)
}

fn project_field(field: &checked::FieldInfo) -> ExecutableFieldInfo {
    ExecutableFieldInfo {
        name: field.name.clone(),
        type_name: field.ty.to_string(),
    }
}

fn project_block(block: &checked::HirBlock) -> ExecutableBlock {
    ExecutableBlock {
        statements: block.statements.iter().map(project_stmt).collect(),
    }
}

fn project_stmt(stmt: &checked::HirStmt) -> ExecutableStmt {
    match stmt {
        checked::HirStmt::Let {
            name,
            value,
            is_async,
            ..
        } => ExecutableStmt::Let {
            name: name.clone(),
            value: value.as_ref().map(project_expr),
            is_async: *is_async,
        },
        checked::HirStmt::Return { value, .. } => ExecutableStmt::Return {
            value: value.as_ref().map(project_expr),
        },
        checked::HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => ExecutableStmt::With {
            resource: project_expr(resource),
            binding: binding.clone(),
            body: project_block(body),
        },
        checked::HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => ExecutableStmt::If {
            condition: project_expr(condition),
            then_body: project_block(then_body),
            else_body: else_body.as_ref().map(project_block),
        },
        checked::HirStmt::Loop {
            condition, body, ..
        } => ExecutableStmt::Loop {
            condition: condition.as_ref().map(project_expr),
            body: project_block(body),
        },
        checked::HirStmt::For {
            binding,
            iterable,
            is_async,
            body,
            ..
        } => ExecutableStmt::For {
            binding: binding.clone(),
            iterable: project_expr(iterable),
            is_async: *is_async,
            body: project_block(body),
        },
        checked::HirStmt::Match { value, arms, .. } => ExecutableStmt::Match {
            value: project_expr(value),
            arms: arms.iter().map(project_match_arm).collect(),
        },
        checked::HirStmt::Select { arms, .. } => ExecutableStmt::Select {
            arms: arms
                .iter()
                .map(|arm| ExecutableSelectArm {
                    binding: arm.binding.clone(),
                    operation: project_expr(&arm.operation),
                    body: project_block(&arm.body),
                })
                .collect(),
        },
        checked::HirStmt::Assign { target, value, .. } => ExecutableStmt::Assign {
            target: project_expr(target),
            value: project_expr(value),
        },
        checked::HirStmt::Break(_) => ExecutableStmt::Break,
        checked::HirStmt::Continue(_) => ExecutableStmt::Continue,
        checked::HirStmt::Expr(expr) => ExecutableStmt::Expr(project_expr(expr)),
        checked::HirStmt::Unknown(_) => ExecutableStmt::Unknown,
    }
}

fn project_match_arm(arm: &checked::HirMatchArm) -> ExecutableMatchArm {
    ExecutableMatchArm {
        pattern: project_pattern(&arm.pattern),
        guard: arm.guard.as_ref().map(project_expr),
        body: project_block(&arm.body),
    }
}

fn project_expr(expr: &checked::HirExpr) -> ExecutableExpr {
    match expr {
        checked::HirExpr::Ident {
            name, type_name, ..
        } => ExecutableExpr::Ident {
            name: name.clone(),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Number { value, .. } => ExecutableExpr::Number {
            value: value.clone(),
        },
        checked::HirExpr::String { value, .. } => ExecutableExpr::String {
            value: value.clone(),
        },
        checked::HirExpr::Char { value, .. } => ExecutableExpr::Char {
            value: value.clone(),
        },
        checked::HirExpr::ObjectLiteral {
            fields, type_name, ..
        } => ExecutableExpr::ObjectLiteral {
            fields: fields
                .iter()
                .map(|field| ExecutableObjectLiteralField {
                    name: field.name.clone(),
                    value: project_expr(&field.value),
                })
                .collect(),
            type_name: type_name.clone(),
        },
        checked::HirExpr::MapLiteral {
            entries, type_name, ..
        } => ExecutableExpr::MapLiteral {
            entries: entries
                .iter()
                .map(|entry| ExecutableMapLiteralEntry {
                    key: project_expr(&entry.key),
                    value: project_expr(&entry.value),
                })
                .collect(),
            type_name: type_name.clone(),
        },
        checked::HirExpr::ArrayLiteral {
            items, type_name, ..
        } => ExecutableExpr::ArrayLiteral {
            items: items.iter().map(project_expr).collect(),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Binary {
            op, left, right, ..
        } => ExecutableExpr::Binary {
            op: project_binary(*op),
            left: Box::new(project_expr(left)),
            right: Box::new(project_expr(right)),
        },
        checked::HirExpr::Field {
            base, name, access, ..
        } => ExecutableExpr::Field {
            base: Box::new(project_expr(base)),
            name: name.clone(),
            access: ExecutableFieldAccess {
                base_type: access.base_type.clone(),
                type_name: access.type_name.clone(),
            },
        },
        checked::HirExpr::Index { base, index, .. } => ExecutableExpr::Index {
            base: Box::new(project_expr(base)),
            index: Box::new(project_expr(index)),
        },
        checked::HirExpr::Call {
            callee,
            receiver,
            args,
            type_name,
            ..
        } => ExecutableExpr::Call {
            callee: project_callee(callee),
            receiver: receiver.as_ref().map(|receiver| ExecutableCallReceiver {
                value: Box::new(project_expr(&receiver.value)),
                effect: project_param_effect(receiver.effect),
                type_name: receiver.type_name.clone(),
                resolved_namespace: receiver.resolved_namespace.clone(),
            }),
            args: args
                .iter()
                .map(|arg| ExecutableCallArg {
                    name: arg.name.clone(),
                    value: project_expr(&arg.value),
                    parameter_index: arg.parameter_index,
                    evaluation_index: arg.evaluation_index,
                })
                .collect(),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Effect {
            effect,
            value,
            type_name,
            ..
        } => ExecutableExpr::Effect {
            effect: project_param_effect(*effect),
            value: Box::new(project_expr(value)),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Manage {
            value, type_name, ..
        } => ExecutableExpr::Manage {
            value: Box::new(project_expr(value)),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Spawn {
            value, type_name, ..
        } => ExecutableExpr::Spawn {
            value: Box::new(project_expr(value)),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Await {
            value, type_name, ..
        } => ExecutableExpr::Await {
            value: Box::new(project_expr(value)),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Try {
            value, type_name, ..
        } => ExecutableExpr::Try {
            value: Box::new(project_expr(value)),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Closure {
            params,
            captures,
            explicit,
            body,
            ..
        } => ExecutableExpr::Closure {
            params: params.clone(),
            captures: captures
                .iter()
                .map(|capture| ExecutableClosureCapture {
                    name: capture.name.clone(),
                })
                .collect(),
            explicit: *explicit,
            body: project_block(body),
        },
        checked::HirExpr::Match {
            value,
            arms,
            type_name,
            ..
        } => ExecutableExpr::Match {
            value: Box::new(project_expr(value)),
            arms: arms.iter().map(project_match_arm).collect(),
            type_name: type_name.clone(),
        },
        checked::HirExpr::Unknown(_) => ExecutableExpr::Unknown,
    }
}

fn project_param_effect(effect: checked::ParamEffect) -> ParamEffect {
    match effect {
        checked::ParamEffect::Read => ParamEffect::Read,
        checked::ParamEffect::Mut => ParamEffect::Mut,
        checked::ParamEffect::Take => ParamEffect::Take,
    }
}

fn project_binary(op: syntax::BinaryOp) -> BinaryOp {
    match op {
        syntax::BinaryOp::Add => BinaryOp::Add,
        syntax::BinaryOp::Subtract => BinaryOp::Subtract,
        syntax::BinaryOp::Multiply => BinaryOp::Multiply,
        syntax::BinaryOp::Divide => BinaryOp::Divide,
        syntax::BinaryOp::Modulo => BinaryOp::Modulo,
        syntax::BinaryOp::BitAnd => BinaryOp::BitAnd,
        syntax::BinaryOp::BitOr => BinaryOp::BitOr,
        syntax::BinaryOp::BitXor => BinaryOp::BitXor,
        syntax::BinaryOp::ShiftLeft => BinaryOp::ShiftLeft,
        syntax::BinaryOp::ShiftRight => BinaryOp::ShiftRight,
        syntax::BinaryOp::Equal => BinaryOp::Equal,
        syntax::BinaryOp::NotEqual => BinaryOp::NotEqual,
        syntax::BinaryOp::Less => BinaryOp::Less,
        syntax::BinaryOp::LessEqual => BinaryOp::LessEqual,
        syntax::BinaryOp::Greater => BinaryOp::Greater,
        syntax::BinaryOp::GreaterEqual => BinaryOp::GreaterEqual,
        syntax::BinaryOp::LogicalAnd => BinaryOp::LogicalAnd,
        syntax::BinaryOp::LogicalOr => BinaryOp::LogicalOr,
    }
}

fn project_callee(callee: &syntax::Callee) -> Callee {
    match callee {
        syntax::Callee::Name(name) => Callee::Name(name.clone()),
        syntax::Callee::Qualified { namespace, name } => Callee::Qualified {
            namespace: namespace.clone(),
            name: name.clone(),
        },
        syntax::Callee::ReceiverCall { method, .. } => Callee::ReceiverCall {
            method: method.clone(),
        },
    }
}

fn project_pattern(pattern: &syntax::MatchPattern) -> MatchPattern {
    match pattern {
        syntax::MatchPattern::Binding { name, .. } => MatchPattern::Binding { name: name.clone() },
        syntax::MatchPattern::Variant { name, bindings, .. } => MatchPattern::Variant {
            name: name.clone(),
            bindings: bindings.iter().map(project_pattern).collect(),
        },
        syntax::MatchPattern::Struct {
            name,
            fields,
            has_rest,
            ..
        } => MatchPattern::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|field| MatchFieldPattern {
                    name: field.name.clone(),
                    binding: field.binding.clone(),
                    pattern: field
                        .pattern
                        .as_ref()
                        .map(|pattern| Box::new(project_pattern(pattern))),
                    ignored: field.ignored,
                })
                .collect(),
            has_rest: *has_rest,
        },
        syntax::MatchPattern::Literal { value, .. } => MatchPattern::Literal {
            value: match value {
                syntax::MatchLiteral::Int(value) => MatchLiteral::Int(value.clone()),
                syntax::MatchLiteral::String(value) => MatchLiteral::String(value.clone()),
                syntax::MatchLiteral::Char(value) => MatchLiteral::Char(value.clone()),
                syntax::MatchLiteral::Bool(value) => MatchLiteral::Bool(*value),
            },
        },
        syntax::MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } => MatchPattern::List {
            prefix: prefix.iter().map(project_pattern).collect(),
            rest: rest.clone(),
            suffix: suffix.iter().map(project_pattern).collect(),
        },
        syntax::MatchPattern::Wildcard(_) => MatchPattern::Wildcard,
    }
}
