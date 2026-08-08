use std::collections::BTreeMap;

use rsscript_abi_model::{ExternalSymbol, FunctionSignature};

fn type_root_name(name: &str) -> &str {
    name.split('<').next().unwrap_or(name).trim()
}

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
    #[doc(hidden)]
    pub fn insert_type(&mut self, value: ExecutableTypeInfo) {
        self.types.insert(value.name.clone(), value);
    }

    #[doc(hidden)]
    pub fn insert_resource_drop(&mut self, name: String, body: ExecutableBlock) {
        self.resource_drop_bodies.insert(name, body);
    }

    #[doc(hidden)]
    pub fn insert_function(&mut self, name: String, function: ExecutableFunction) {
        self.functions.insert(name, function);
    }

    #[doc(hidden)]
    pub fn insert_signature(&mut self, name: String, signature: ExecutableSignature) {
        self.signatures.insert(name, signature);
    }

    #[doc(hidden)]
    pub fn insert_protocol_targets(
        &mut self,
        key: (String, String),
        targets: Vec<(String, String)>,
    ) {
        self.protocol_dispatch.entry(key).or_insert(targets);
    }

    #[doc(hidden)]
    pub fn insert_sum_variant(
        &mut self,
        variant: String,
        owner: String,
        fields: Vec<ExecutableFieldInfo>,
    ) {
        self.sum_variant_types.insert(variant.clone(), owner);
        self.sum_variant_fields.insert(variant, fields);
    }

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
