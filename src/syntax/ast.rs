use crate::diagnostic::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileFeature {
    Local,
    Native,
    Unsafe,
    Async,
    Device,
    Ffi,
    Reflection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownFileFeature {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateFileFeature {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFeatureScope {
    pub file: String,
    pub features: Vec<FileFeature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub features: Vec<FileFeature>,
    pub feature_scopes: Vec<FileFeatureScope>,
    pub unknown_features: Vec<UnknownFileFeature>,
    pub duplicate_features: Vec<DuplicateFileFeature>,
    pub feature_spans: Vec<Span>,
    pub profile_spans: Vec<Span>,
    pub unknown_top_level_spans: Vec<Span>,
    pub malformed_declaration_spans: Vec<Span>,
    pub protocols: Vec<ProtocolDecl>,
    pub protocol_impls: Vec<ProtocolImpl>,
    pub items: Vec<Item>,
}

impl Program {
    pub fn has_feature(&self, feature: FileFeature) -> bool {
        self.features.contains(&feature)
    }

    pub fn file_has_feature(&self, file: &str, feature: FileFeature) -> bool {
        self.feature_scopes
            .iter()
            .any(|scope| scope.file == file && scope.features.contains(&feature))
    }

    pub fn local_capability_enabled(&self) -> bool {
        self.has_feature(FileFeature::Local)
    }
}

pub fn merge_programs(programs: impl IntoIterator<Item = Program>) -> Program {
    let mut features = Vec::new();
    let mut feature_scopes: Vec<FileFeatureScope> = Vec::new();
    let mut unknown_features = Vec::new();
    let mut duplicate_features = Vec::new();
    let mut feature_spans = Vec::new();
    let mut profile_spans = Vec::new();
    let mut unknown_top_level_spans = Vec::new();
    let mut malformed_declaration_spans = Vec::new();
    let mut protocols = Vec::new();
    let mut protocol_impls = Vec::new();
    let mut items = Vec::new();

    for program in programs {
        for feature in program.features {
            if !features.contains(&feature) {
                features.push(feature);
            }
        }
        for scope in program.feature_scopes {
            if let Some(existing) = feature_scopes
                .iter_mut()
                .find(|existing| existing.file == scope.file)
            {
                for feature in scope.features {
                    if !existing.features.contains(&feature) {
                        existing.features.push(feature);
                    }
                }
            } else {
                feature_scopes.push(scope);
            }
        }
        unknown_features.extend(program.unknown_features);
        duplicate_features.extend(program.duplicate_features);
        unknown_top_level_spans.extend(program.unknown_top_level_spans);
        malformed_declaration_spans.extend(program.malformed_declaration_spans);
        if program.feature_spans.len() > 1 {
            feature_spans.extend(program.feature_spans);
        }
        profile_spans.extend(program.profile_spans);
        protocols.extend(program.protocols);
        protocol_impls.extend(program.protocol_impls);
        items.extend(program.items);
    }

    Program {
        features,
        feature_scopes,
        unknown_features,
        duplicate_features,
        feature_spans,
        profile_spans,
        unknown_top_level_spans,
        malformed_declaration_spans,
        protocols,
        protocol_impls,
        items,
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Module(ModuleDecl),
    Use(UseDecl),
    Type(TypeDecl),
    SumType(SumTypeDecl),
    TypeAlias(TypeAliasDecl),
    Const(ConstDecl),
    Function(FunctionDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDecl {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Struct,
    Resource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDecl {
    pub kind: TypeKind,
    pub name: String,
    pub is_public: bool,
    pub is_opaque: bool,
    pub type_params: Vec<GenericParam>,
    pub malformed_generic_param_spans: Vec<Span>,
    pub derives: Vec<String>,
    pub fields: Vec<FieldDecl>,
    pub malformed_field_spans: Vec<Span>,
    pub drop_body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SumTypeDecl {
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub derives: Vec<String>,
    pub variants: Vec<SumVariant>,
    pub is_public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SumVariant {
    pub name: String,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAliasDecl {
    pub name: String,
    pub type_params: Vec<GenericParam>,
    pub target: TypeRef,
    pub is_public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub name: String,
    pub type_annotation: Option<TypeRef>,
    pub value: Expr,
    pub is_public: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParam {
    pub name: String,
    pub bound: Option<GenericBound>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericBound {
    Managed,
    Struct,
    Resource,
    Protocol(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolDecl {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolImpl {
    pub protocol: String,
    pub type_name: String,
    pub mappings: Vec<ProtocolImplMapping>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolImplMapping {
    pub method: String,
    pub target: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeRef,
    pub is_handle: bool,
    pub is_weak: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub name: String,
    pub args: Vec<TypeRef>,
    pub malformed_arg_spans: Vec<Span>,
    pub is_fresh: bool,
    pub is_noescape: bool,
    /// `owned Fn(...)`: an owning/stored closure (e.g. a lazy pool factory that the
    /// pool retains and calls on demand). Unlike `noescape`, it escapes the call
    /// and may consume its captures, so it must capture owned values.
    pub is_owned: bool,
    pub fn_params: Vec<TypeRef>,
    pub fn_return: Option<Box<TypeRef>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub name: String,
    pub is_public: bool,
    pub is_async: bool,
    pub is_native: bool,
    pub has_body: bool,
    pub type_params: Vec<GenericParam>,
    pub malformed_generic_param_spans: Vec<Span>,
    pub params: Vec<Param>,
    pub malformed_param_spans: Vec<Span>,
    pub return_ty: Option<TypeRef>,
    pub returns_fresh: bool,
    pub effects: Vec<EffectDecl>,
    pub malformed_effect_spans: Vec<Span>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub effect: Option<DataEffect>,
    pub ty: TypeRef,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataEffect {
    Read,
    Mut,
    Take,
}

impl DataEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mut => "mut",
            Self::Take => "take",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectDecl {
    Name(String),
    Retains(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub statements: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    Let(LetStmt),
    Return(ReturnStmt),
    With(WithStmt),
    MalformedWith(Span),
    If(IfStmt),
    MalformedIf(Span),
    Loop(LoopStmt),
    MalformedLoop(Span),
    For(ForStmt),
    MalformedFor(Span),
    Match(MatchStmt),
    MalformedMatch(Span),
    TaskGroup(TaskGroupStmt),
    Select(SelectStmt),
    Break(Span),
    Continue(Span),
    LetElse(LetElseStmt),
    Assign(AssignStmt),
    Expr(Expr),
    Unknown(Span),
}

/// Controlled ordinary assignment to a mutable place: `x = e`, `obj.field = e`,
/// or `list[i] = e`. The `target` is validated to be a place during checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignStmt {
    pub target: Expr,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LetKind {
    Managed,
    Local,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetStmt {
    pub kind: LetKind,
    pub name: String,
    pub type_annotation: Option<TypeRef>,
    pub value: Option<Expr>,
    pub is_async: bool,
    pub is_mut: bool,
    pub malformed: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnStmt {
    pub value: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithStmt {
    pub resource: Expr,
    pub binding: String,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfStmt {
    pub condition: Expr,
    pub then_body: Block,
    pub else_body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopStmt {
    pub condition: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForStmt {
    pub binding: String,
    pub iterable: Expr,
    pub body: Block,
    pub is_async: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LetElseStmt {
    pub pattern: MatchPattern,
    pub value: Expr,
    pub else_body: Block,
    pub binding_name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGroupStmt {
    pub body: Block,
    pub span: Span,
}

/// `select { binding = await <op> => { body } ... }` — wait on several async
/// operations and run the body of whichever becomes ready first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectStmt {
    pub arms: Vec<SelectArm>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectArm {
    /// The name bound to the awaited result inside `body`, or `_` to ignore it.
    pub binding: String,
    /// The awaited operation, i.e. an `await <pending>` expression.
    pub operation: Expr,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchStmt {
    pub value: Expr,
    pub arms: Vec<MatchArm>,
    pub malformed_arm_spans: Vec<Span>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchPattern {
    Variant {
        name: String,
        binding: Option<String>,
        span: Span,
    },
    Wildcard(Span),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Ident(String, Span),
    Number(String, Span),
    String(String, Span),
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        name: String,
        span: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Callee,
        args: Vec<CallArg>,
        span: Span,
    },
    Effect {
        effect: DataEffect,
        value: Box<Expr>,
        span: Span,
    },
    Manage {
        value: Box<Expr>,
        span: Span,
    },
    Spawn {
        value: Box<Expr>,
        span: Span,
    },
    Await {
        value: Box<Expr>,
        span: Span,
    },
    Try {
        value: Box<Expr>,
        span: Span,
    },
    Closure {
        params: Vec<String>,
        body: Block,
        span: Span,
    },
    Match {
        value: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Unknown(Span),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
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
    Qualified {
        namespace: String,
        name: String,
    },
    /// Receiver-call shorthand: `<effect> receiver.method(args)`
    /// Desugars to `Type.method(self: <effect> receiver, args)`.
    ReceiverCall {
        receiver: String,
        method: String,
        effect: DataEffect,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallArg {
    pub name: Option<String>,
    pub value: Expr,
    pub malformed: bool,
    pub span: Span,
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Self::Ident(_, span)
            | Self::Number(_, span)
            | Self::String(_, span)
            | Self::Binary { span, .. }
            | Self::Field { span, .. }
            | Self::Index { span, .. }
            | Self::Call { span, .. }
            | Self::Effect { span, .. }
            | Self::Manage { span, .. }
            | Self::Spawn { span, .. }
            | Self::Await { span, .. }
            | Self::Try { span, .. }
            | Self::Closure { span, .. }
            | Self::Match { span, .. }
            | Self::Unknown(span) => span,
        }
    }
}
