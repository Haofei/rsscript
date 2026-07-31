use crate::text_util::{type_arg_names, type_root_name};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::diagnostic::Span;
use crate::interfaces::{builtin_interfaces, standard_package_interfaces};
use crate::semantic::SemanticTypeFacts;
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FunctionDecl,
    GenericBound, Item, LetKind, MatchPattern, Param, Program as SyntaxProgram, ProtocolImpl, Stmt,
    TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamEffect {
    Read,
    Mut,
    Take,
}

impl ParamEffect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mut => "mut",
            Self::Take => "take",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirClosureCapture {
    pub effect: ParamEffect,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSig {
    pub name: String,
    pub effect: Option<ParamEffect>,
    pub type_name: String,
    /// The parameter's default value expression, if it has one (`name: T = expr`).
    pub default: Option<crate::syntax::ast::Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub namespace: Option<String>,
    pub name: String,
    pub is_public: bool,
    pub is_async: bool,
    pub is_native: bool,
    pub type_params: Box<[String]>,
    pub type_param_bounds: Vec<Option<GenericBound>>,
    pub params: Vec<ParamSig>,
    pub return_type: Option<String>,
    pub returns_fresh: bool,
    pub effects: Vec<String>,
    pub retained_params: HashSet<String>,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HirProtocolImpl {
    protocol: String,
    type_name: String,
    mappings: Vec<crate::syntax::ast::ProtocolImplMapping>,
    is_current_program: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirTypeKind {
    Class,
    Struct,
    Resource,
    Sum,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    pub type_name: String,
    pub is_handle: bool,
    pub is_weak: bool,
    /// Default value for the field, if declared (`name: Type = <expr>`); lets a
    /// constructor call omit the field and have it filled.
    pub default: Option<crate::syntax::ast::Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub kind: HirTypeKind,
    pub type_params: Box<[String]>,
    pub fields_ordered: Vec<FieldInfo>,
    pub fields: HashMap<String, FieldInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateSymbolKind {
    Function,
    Type,
    Constructor,
    Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateSymbol {
    pub kind: DuplicateSymbolKind,
    pub name: String,
    pub first_span: Span,
    pub duplicate_span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedCalleeKind {
    UserFunction,
    BuiltinFunction,
    Constructor { type_kind: HirTypeKind },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallResolution {
    Resolved {
        signature: FunctionSig,
        kind: ResolvedCalleeKind,
    },
    EnumVariant,
    Ambiguous {
        candidates: Vec<String>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirCallSite {
    pub function_name: String,
    pub callee: Callee,
    pub span: Span,
    pub resolution: CallResolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirFeatureUseKind {
    LocalLet,
    LocalClosure,
    Manage,
    Take,
    Native,
    Unsafe,
    Async,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirFeatureUse {
    pub function_name: Option<String>,
    pub kind: HirFeatureUseKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirBindingKind {
    Param,
    ManagedLet,
    LocalLet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirBinding {
    pub function_name: String,
    pub name: String,
    pub kind: HirBindingKind,
    pub effect: Option<ParamEffect>,
    pub span: Span,
    pub type_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirFieldAccess {
    pub function_name: String,
    pub name: String,
    pub span: Span,
    pub base_type: Option<String>,
    pub type_name: Option<String>,
    pub is_handle: bool,
    pub is_weak: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirEffectEventKind {
    Manage,
    Take,
    Retain { callee: String, param: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirEffectEvent {
    pub function_name: String,
    pub kind: HirEffectEventKind,
    pub binding_name: String,
    pub span: Span,
    pub value_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirReturnProof {
    NoValue,
    Ident {
        name: String,
    },
    StructConstructor,
    FreshCall,
    /// A literal (string, number, or boolean). A literal owns no borrowed or
    /// aliased resource, so returning it is trivially fresh.
    Literal,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirReturn {
    pub function_name: String,
    pub span: Span,
    pub proof: HirReturnProof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirBlock {
    pub statements: Vec<HirStmt>,
    pub span: Span,
}

// HIR nodes are built once per compile and matched by reference, never kept in
// large hot collections, so the size spread between variants doesn't matter;
// boxing the big arms would churn dozens of `match` sites for no runtime gain.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirStmt {
    Let {
        kind: HirBindingKind,
        name: String,
        value: Option<HirExpr>,
        type_name: Option<String>,
        value_type_name: Option<String>,
        is_async: bool,
        span: Span,
    },
    Return {
        value: Option<HirExpr>,
        proof: HirReturnProof,
        span: Span,
    },
    With {
        resource: HirExpr,
        binding: String,
        body: HirBlock,
        span: Span,
    },
    If {
        condition: HirExpr,
        then_body: HirBlock,
        else_body: Option<HirBlock>,
        span: Span,
    },
    Loop {
        condition: Option<HirExpr>,
        body: HirBlock,
        span: Span,
    },
    For {
        binding: String,
        iterable: HirExpr,
        iterable_type_name: Option<String>,
        item_type_name: Option<String>,
        is_async: bool,
        body: HirBlock,
        span: Span,
    },
    Match {
        value: HirExpr,
        scrutinee_effect: Option<DataEffect>,
        arms: Vec<HirMatchArm>,
        span: Span,
    },
    Select {
        arms: Vec<HirSelectArm>,
        span: Span,
    },
    /// Controlled reassignment to an existing binding (spec: `x = expr`).
    /// The checker validates assignment legality at the AST level and only
    /// needs the RHS for ownership/use analysis, so checker passes treat this
    /// exactly like `Expr(value)`. The `target` is carried purely so executable
    /// backends (interpreter) know which binding to store into.
    Assign {
        target: HirExpr,
        value: HirExpr,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Expr(HirExpr),
    Unknown(Span),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirMatchArm {
    pub pattern: MatchPattern,
    pub guard: Option<HirExpr>,
    pub body: HirBlock,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirSelectArm {
    pub binding: String,
    pub operation: HirExpr,
    pub body: HirBlock,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirCallReceiver {
    pub value: Box<HirExpr>,
    pub effect: ParamEffect,
    pub type_name: Option<String>,
    pub resolved_namespace: Option<String>,
}

// See `HirStmt`: boxing for size parity isn't worth the match-site churn here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirExpr {
    Ident {
        name: String,
        type_name: Option<String>,
        span: Span,
    },
    Number {
        value: String,
        span: Span,
    },
    String {
        value: String,
        span: Span,
    },
    Char {
        value: String,
        span: Span,
    },
    ObjectLiteral {
        fields: Vec<HirObjectLiteralField>,
        type_name: Option<String>,
        span: Span,
    },
    MapLiteral {
        entries: Vec<HirMapLiteralEntry>,
        type_name: Option<String>,
        span: Span,
    },
    ArrayLiteral {
        items: Vec<HirExpr>,
        type_name: Option<String>,
        span: Span,
    },
    Binary {
        op: BinaryOp,
        left: Box<HirExpr>,
        right: Box<HirExpr>,
        span: Span,
    },
    Field {
        base: Box<HirExpr>,
        name: String,
        access: HirFieldAccess,
        span: Span,
    },
    Index {
        base: Box<HirExpr>,
        index: Box<HirExpr>,
        span: Span,
    },
    Call {
        callee: Callee,
        receiver: Option<HirCallReceiver>,
        args: Vec<HirCallArg>,
        resolution: CallResolution,
        events: Vec<HirEffectEvent>,
        type_name: Option<String>,
        span: Span,
    },
    Effect {
        effect: ParamEffect,
        value: Box<HirExpr>,
        events: Vec<HirEffectEvent>,
        type_name: Option<String>,
        span: Span,
    },
    Manage {
        value: Box<HirExpr>,
        events: Vec<HirEffectEvent>,
        type_name: Option<String>,
        span: Span,
    },
    Spawn {
        value: Box<HirExpr>,
        type_name: Option<String>,
        span: Span,
    },
    Await {
        value: Box<HirExpr>,
        type_name: Option<String>,
        span: Span,
    },
    Try {
        value: Box<HirExpr>,
        type_name: Option<String>,
        span: Span,
    },
    Closure {
        params: Vec<String>,
        captures: Vec<HirClosureCapture>,
        declared_effects: Vec<String>,
        explicit: bool,
        body: HirBlock,
        span: Span,
    },
    Match {
        value: Box<HirExpr>,
        scrutinee_effect: Option<DataEffect>,
        arms: Vec<HirMatchArm>,
        type_name: Option<String>,
        span: Span,
    },
    Unknown(Span),
}

pub(crate) fn number_literal_type_name(value: &str) -> &'static str {
    if value.contains('.') { "Float" } else { "Int" }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirObjectLiteralField {
    pub name: String,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirMapLiteralEntry {
    pub key: HirExpr,
    pub value: HirExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirCallArg {
    pub name: Option<String>,
    pub value: HirExpr,
    /// Declared parameter slot selected by call binding. `None` is retained only
    /// for unresolved/malformed calls so diagnostics can continue.
    pub parameter_index: Option<usize>,
    /// Source-language evaluation order. Explicit arguments come first in their
    /// written order; synthesized defaults follow in declaration order.
    pub evaluation_index: usize,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HirFunctionBody {
    pub function_name: String,
    pub block: Option<HirBlock>,
    pub bindings: Vec<HirBinding>,
    pub call_sites: Vec<HirCallSite>,
    pub field_accesses: Vec<HirFieldAccess>,
    pub effect_events: Vec<HirEffectEvent>,
    pub returns: Vec<HirReturn>,
}

#[derive(Debug, Default)]
pub struct Hir {
    semantic_types: Arc<SemanticTypeFacts>,
    signatures: HashMap<String, FunctionSig>,
    types: HashMap<String, TypeInfo>,
    type_aliases: HashMap<String, (Vec<String>, String)>,
    // user-declared types whose derive list includes `Clone` (gates the synthesized `.clone()`)
    clone_types: HashSet<String>,
    fields_by_name: HashMap<String, Vec<FieldInfo>>,
    sum_variant_types: HashMap<String, String>,
    sum_variant_fields: HashMap<String, Vec<FieldInfo>>,
    duplicate_symbols: Vec<DuplicateSymbol>,
    call_sites: Vec<HirCallSite>,
    bindings: Vec<HirBinding>,
    field_accesses: Vec<HirFieldAccess>,
    effect_events: Vec<HirEffectEvent>,
    returns: Vec<HirReturn>,
    feature_uses: Vec<HirFeatureUse>,
    function_bodies: HashMap<String, HirFunctionBody>,
    resource_drop_bodies: HashMap<String, HirBlock>,
    protocol_impls: Vec<HirProtocolImpl>,
    /// Top-level `const` values (name → literal initializer). References to a
    /// const are inlined to this literal during expression lowering, so the
    /// register VM (which has no global/const slots) resolves them.
    const_values: HashMap<String, Expr>,
}

mod infer;
mod lower;

pub(crate) use infer::infer_hir_expr_type;
pub(crate) use lower::assign_target_reads;

// Re-exported so each sibling submodule (which `use super::*`) can reach the
// helpers defined in the other. These moved across the module boundary during
// the hir.rs split and keep their original module-private reach via `pub(super)`
// plus this re-export.
use infer::{
    capability_protocol, classify_return_expr, infer_closure_return_type, list_element_type,
    match_pattern_binding_type, match_pattern_binding_types, stream_item_type,
    substituted_field_type,
};
use lower::callee_name;

#[cfg(test)]
mod tests;
