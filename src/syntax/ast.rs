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
pub struct Program {
    pub features: Vec<FileFeature>,
    pub unknown_features: Vec<UnknownFileFeature>,
    pub duplicate_features: Vec<DuplicateFileFeature>,
    pub feature_spans: Vec<Span>,
    pub profile_spans: Vec<Span>,
    pub unknown_top_level_spans: Vec<Span>,
    pub malformed_declaration_spans: Vec<Span>,
    pub items: Vec<Item>,
}

impl Program {
    pub fn has_feature(&self, feature: FileFeature) -> bool {
        self.features.contains(&feature)
    }

    pub fn local_capability_enabled(&self) -> bool {
        self.has_feature(FileFeature::Local)
    }
}

pub fn merge_programs(programs: impl IntoIterator<Item = Program>) -> Program {
    let mut features = Vec::new();
    let mut unknown_features = Vec::new();
    let mut duplicate_features = Vec::new();
    let mut feature_spans = Vec::new();
    let mut profile_spans = Vec::new();
    let mut unknown_top_level_spans = Vec::new();
    let mut malformed_declaration_spans = Vec::new();
    let mut items = Vec::new();

    for program in programs {
        for feature in program.features {
            if !features.contains(&feature) {
                features.push(feature);
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
        items.extend(program.items);
    }

    Program {
        features,
        unknown_features,
        duplicate_features,
        feature_spans,
        profile_spans,
        unknown_top_level_spans,
        malformed_declaration_spans,
        items,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Type(TypeDecl),
    Function(FunctionDecl),
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
    pub is_opaque: bool,
    pub type_params: Vec<GenericParam>,
    pub malformed_generic_param_spans: Vec<Span>,
    pub fields: Vec<FieldDecl>,
    pub malformed_field_spans: Vec<Span>,
    pub drop_body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParam {
    pub name: String,
    pub bound: Option<GenericBound>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericBound {
    Managed,
    Struct,
    Resource,
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
    pub is_noescape: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub name: String,
    pub is_public: bool,
    pub is_async: bool,
    pub is_native: bool,
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
    Match(MatchStmt),
    MalformedMatch(Span),
    Break(Span),
    Continue(Span),
    Expr(Expr),
    Unknown(Span),
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
    pub value: Option<Expr>,
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
        body: Block,
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
    Qualified { namespace: String, name: String },
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
            | Self::Unknown(span) => span,
        }
    }
}
