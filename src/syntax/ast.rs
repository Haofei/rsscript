use crate::diagnostic::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Managed,
    UsesLocal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub mode: Option<FileMode>,
    pub items: Vec<Item>,
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
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub name: String,
    pub ty: TypeRef,
    pub is_handle: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub name: String,
    pub args: Vec<TypeRef>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_ty: Option<TypeRef>,
    pub returns_fresh: bool,
    pub effects: Vec<EffectDecl>,
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
    If(IfStmt),
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
pub enum Expr {
    Ident(String, Span),
    Number(String, Span),
    String(String, Span),
    Field {
        base: Box<Expr>,
        name: String,
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
    Closure {
        body: Block,
        span: Span,
    },
    Unknown(Span),
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
    pub span: Span,
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Self::Ident(_, span)
            | Self::Number(_, span)
            | Self::String(_, span)
            | Self::Field { span, .. }
            | Self::Call { span, .. }
            | Self::Effect { span, .. }
            | Self::Manage { span, .. }
            | Self::Closure { span, .. }
            | Self::Unknown(span) => span,
        }
    }
}
