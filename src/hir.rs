use std::collections::{HashMap, HashSet};

use crate::diagnostic::Span;
use crate::interfaces::builtin_interfaces;
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FunctionDecl, Item,
    LetKind, MatchPattern, Param, Program as SyntaxProgram, Stmt, TypeDecl, TypeKind, TypeRef,
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
pub struct ParamSig {
    pub name: String,
    pub effect: Option<ParamEffect>,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSig {
    pub namespace: Option<String>,
    pub name: String,
    pub is_async: bool,
    pub params: Vec<ParamSig>,
    pub return_type: Option<String>,
    pub returns_fresh: bool,
    pub effects: Vec<String>,
    pub retained_params: HashSet<String>,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HirTypeKind {
    Class,
    Struct,
    Resource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    pub type_name: String,
    pub is_handle: bool,
    pub is_weak: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub kind: HirTypeKind,
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
    ResourcePool,
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
    Ident { name: String },
    StructConstructor,
    FreshCall,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HirStmt {
    Let {
        kind: HirBindingKind,
        name: String,
        value: Option<HirExpr>,
        type_name: Option<String>,
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
    Match {
        value: HirExpr,
        arms: Vec<HirMatchArm>,
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
    pub body: HirBlock,
    pub span: Span,
}

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
    Try {
        value: Box<HirExpr>,
        type_name: Option<String>,
        span: Span,
    },
    Closure {
        body: HirBlock,
        span: Span,
    },
    Unknown(Span),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HirCallArg {
    pub name: Option<String>,
    pub value: HirExpr,
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
    signatures: HashMap<String, FunctionSig>,
    types: HashMap<String, TypeInfo>,
    fields_by_name: HashMap<String, Vec<FieldInfo>>,
    duplicate_symbols: Vec<DuplicateSymbol>,
    call_sites: Vec<HirCallSite>,
    bindings: Vec<HirBinding>,
    field_accesses: Vec<HirFieldAccess>,
    effect_events: Vec<HirEffectEvent>,
    returns: Vec<HirReturn>,
    feature_uses: Vec<HirFeatureUse>,
    function_bodies: HashMap<String, HirFunctionBody>,
}

impl Hir {
    pub fn from_syntax(program: &SyntaxProgram) -> Self {
        Self::from_syntax_with_interfaces(program, &[])
    }

    pub fn from_syntax_with_interfaces(
        program: &SyntaxProgram,
        interfaces: &[SyntaxProgram],
    ) -> Self {
        let mut hir = Self::default();
        hir.insert_builtin_interfaces();
        let mut type_symbols: HashMap<String, (DuplicateSymbolKind, Span)> = HashMap::new();
        let mut callable_symbols: HashMap<String, (DuplicateSymbolKind, Span)> = HashMap::new();
        for interface in interfaces {
            hir.collect_item_signatures(interface, &mut type_symbols, &mut callable_symbols);
        }
        hir.collect_item_signatures(program, &mut type_symbols, &mut callable_symbols);
        hir.collect_body_facts(program);
        hir
    }

    fn collect_item_signatures(
        &mut self,
        program: &SyntaxProgram,
        type_symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
        callable_symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
    ) {
        for item in &program.items {
            match item {
                Item::Function(function) => {
                    record_duplicate_symbol(
                        &mut self.duplicate_symbols,
                        callable_symbols,
                        DuplicateSymbolKind::Function,
                        &function.name,
                        &function.span,
                    );
                    self.insert_function(function_sig_from_decl(function, false));
                }
                Item::Type(type_decl) => {
                    record_duplicate_fields(&mut self.duplicate_symbols, type_decl);
                    record_duplicate_symbol(
                        &mut self.duplicate_symbols,
                        type_symbols,
                        DuplicateSymbolKind::Type,
                        &type_decl.name,
                        &type_decl.span,
                    );
                    record_duplicate_symbol(
                        &mut self.duplicate_symbols,
                        callable_symbols,
                        DuplicateSymbolKind::Constructor,
                        &type_decl.name,
                        &type_decl.span,
                    );
                    self.insert_type(type_info_from_decl(type_decl));
                }
            }
        }
    }

    pub fn resolve_function(&self, namespace: Option<&str>, name: &str) -> Option<&FunctionSig> {
        if let Some(namespace) = namespace
            && let Some(signature) = self.signatures.get(&qualified_key(namespace, name))
        {
            return Some(signature);
        }
        if let Some(namespace) = namespace {
            let namespace = type_root_name(namespace);
            if let Some(signature) = self.signatures.get(&qualified_key(namespace, name)) {
                return Some(signature);
            }
        }
        namespace
            .is_none()
            .then(|| self.signatures.get(name))
            .flatten()
    }

    pub fn type_info(&self, name: &str) -> Option<&TypeInfo> {
        self.types.get(type_root_name(name))
    }

    pub fn type_kind(&self, name: &str) -> Option<HirTypeKind> {
        self.type_info(name).map(|info| info.kind)
    }

    #[cfg(test)]
    fn fields_named(&self, field_name: &str) -> impl Iterator<Item = &FieldInfo> {
        self.fields_by_name
            .get(field_name)
            .into_iter()
            .flat_map(|fields| fields.iter())
    }

    #[cfg(test)]
    fn is_handle_field_name(&self, field_name: &str) -> bool {
        self.fields_named(field_name)
            .any(|field| field.is_handle || field.is_weak)
    }

    pub fn duplicate_symbols(&self) -> &[DuplicateSymbol] {
        &self.duplicate_symbols
    }

    pub fn function_body(&self, function_name: &str) -> Option<&HirFunctionBody> {
        self.function_bodies.get(function_name)
    }

    pub fn feature_uses(&self) -> &[HirFeatureUse] {
        &self.feature_uses
    }

    pub fn resolve_call(&self, callee: &Callee) -> CallResolution {
        let call_name = callee_name(callee);
        if is_enum_variant_call(call_name) {
            return CallResolution::EnumVariant;
        }

        let signature = match callee {
            Callee::Name(name) => self.resolve_function(None, name),
            Callee::Qualified { namespace, name } => self.resolve_function(Some(namespace), name),
        };
        let Some(signature) = signature else {
            return CallResolution::Unknown;
        };
        let kind = match callee {
            Callee::Name(name) => self.type_kind(name).map_or_else(
                || function_kind(signature),
                |type_kind| ResolvedCalleeKind::Constructor { type_kind },
            ),
            Callee::Qualified { .. } => function_kind(signature),
        };

        CallResolution::Resolved {
            signature: signature.clone(),
            kind,
        }
    }

    fn insert_function(&mut self, signature: FunctionSig) {
        let key = match &signature.namespace {
            Some(namespace) => qualified_key(namespace, &signature.name),
            None => signature.name.clone(),
        };
        self.signatures.insert(key, signature);
    }

    fn insert_type(&mut self, type_info: TypeInfo) {
        let constructor = constructor_sig_from_type(&type_info, false);
        for field in type_info.fields.values() {
            self.fields_by_name
                .entry(field.name.clone())
                .or_default()
                .push(field.clone());
        }
        self.types.insert(type_info.name.clone(), type_info);
        self.insert_function(constructor);
    }

    fn insert_builtin_interfaces(&mut self) {
        for (file, source) in builtin_interfaces() {
            let program = parse_source(file, source);
            self.insert_builtin_interface(&program);
        }
    }

    fn insert_builtin_interface(&mut self, program: &SyntaxProgram) {
        for item in &program.items {
            match item {
                Item::Function(function) => {
                    self.insert_function(function_sig_from_decl(function, true));
                }
                Item::Type(type_decl) => {
                    self.insert_builtin_type(type_info_from_decl(type_decl));
                }
            }
        }
    }

    fn insert_builtin_type(&mut self, type_info: TypeInfo) {
        let constructor = constructor_sig_from_type(&type_info, true);
        for field in type_info.fields.values() {
            self.fields_by_name
                .entry(field.name.clone())
                .or_default()
                .push(field.clone());
        }
        self.types.insert(type_info.name.clone(), type_info);
        self.insert_function(constructor);
    }

    fn collect_body_facts(&mut self, program: &SyntaxProgram) {
        let mut facts = BodyFacts::default();
        for item in &program.items {
            match item {
                Item::Function(function) => collect_function_body_facts(self, function, &mut facts),
                Item::Type(type_decl) => collect_type_feature_uses(type_decl, &mut facts),
            }
        }

        self.function_bodies = build_function_bodies(&facts);
        self.call_sites = facts.call_sites;
        self.bindings = facts.bindings;
        self.field_accesses = facts.field_accesses;
        self.effect_events = facts.effect_events;
        self.returns = facts.returns;
        self.feature_uses = facts.feature_uses;
    }
}

fn build_function_bodies(facts: &BodyFacts) -> HashMap<String, HirFunctionBody> {
    let mut bodies = HashMap::<String, HirFunctionBody>::new();
    for (function_name, block) in &facts.blocks {
        body_entry(&mut bodies, function_name).block = Some(block.clone());
    }
    for binding in &facts.bindings {
        body_entry(&mut bodies, &binding.function_name)
            .bindings
            .push(binding.clone());
    }
    for site in &facts.call_sites {
        body_entry(&mut bodies, &site.function_name)
            .call_sites
            .push(site.clone());
    }
    for field in &facts.field_accesses {
        body_entry(&mut bodies, &field.function_name)
            .field_accesses
            .push(field.clone());
    }
    for event in &facts.effect_events {
        body_entry(&mut bodies, &event.function_name)
            .effect_events
            .push(event.clone());
    }
    for return_fact in &facts.returns {
        body_entry(&mut bodies, &return_fact.function_name)
            .returns
            .push(return_fact.clone());
    }
    bodies
}

fn body_entry<'a>(
    bodies: &'a mut HashMap<String, HirFunctionBody>,
    function_name: &str,
) -> &'a mut HirFunctionBody {
    bodies
        .entry(function_name.to_string())
        .or_insert_with(|| HirFunctionBody {
            function_name: function_name.to_string(),
            ..HirFunctionBody::default()
        })
}

#[derive(Default)]
struct BodyFacts {
    blocks: HashMap<String, HirBlock>,
    call_sites: Vec<HirCallSite>,
    bindings: Vec<HirBinding>,
    field_accesses: Vec<HirFieldAccess>,
    effect_events: Vec<HirEffectEvent>,
    returns: Vec<HirReturn>,
    feature_uses: Vec<HirFeatureUse>,
}

fn collect_function_body_facts(hir: &Hir, function: &FunctionDecl, facts: &mut BodyFacts) {
    if function.is_async {
        facts.feature_uses.push(HirFeatureUse {
            function_name: Some(function.name.clone()),
            kind: HirFeatureUseKind::Async,
            span: function.span.clone(),
        });
    }
    for effect in &function.effects {
        if let EffectDecl::Name(name) = effect {
            let kind = match name.as_str() {
                "native" => Some(HirFeatureUseKind::Native),
                "unsafe" => Some(HirFeatureUseKind::Unsafe),
                _ => None,
            };
            if let Some(kind) = kind {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function.name.clone()),
                    kind,
                    span: function.span.clone(),
                });
            }
        }
    }

    let mut value_types = HashMap::new();
    for param in &function.params {
        if param.effect == Some(DataEffect::Take) {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function.name.clone()),
                kind: HirFeatureUseKind::Take,
                span: param.span.clone(),
            });
        }
        collect_feature_uses_in_type_ref(
            Some(&function.name),
            &param.ty,
            HirFeatureUseKind::ResourcePool,
            facts,
        );
        let param_type = type_ref_name(&param.ty);
        value_types.insert(param.name.clone(), param_type.clone());
        facts.bindings.push(HirBinding {
            function_name: function.name.clone(),
            name: param.name.clone(),
            kind: HirBindingKind::Param,
            effect: param.effect.map(param_effect_from_data_effect),
            span: param.span.clone(),
            type_name: Some(param_type),
        });
    }
    if let Some(return_ty) = &function.return_ty {
        collect_feature_uses_in_type_ref(
            Some(&function.name),
            return_ty,
            HirFeatureUseKind::ResourcePool,
            facts,
        );
    }
    let mut lowering_value_types = value_types.clone();
    facts.blocks.insert(
        function.name.clone(),
        lower_hir_block(
            hir,
            &function.name,
            &function.body,
            &mut lowering_value_types,
        ),
    );
    collect_body_facts_in_block(hir, &function.name, &function.body, &mut value_types, facts);
}

fn collect_type_feature_uses(type_decl: &TypeDecl, facts: &mut BodyFacts) {
    for field in &type_decl.fields {
        collect_feature_uses_in_type_ref(None, &field.ty, HirFeatureUseKind::ResourcePool, facts);
    }
}

fn collect_feature_uses_in_type_ref(
    function_name: Option<&str>,
    ty: &TypeRef,
    kind: HirFeatureUseKind,
    facts: &mut BodyFacts,
) {
    if ty.name == "ResourcePool" {
        facts.feature_uses.push(HirFeatureUse {
            function_name: function_name.map(str::to_string),
            kind,
            span: ty.span.clone(),
        });
    }
    for arg in &ty.args {
        collect_feature_uses_in_type_ref(function_name, arg, kind, facts);
    }
}

fn lower_hir_block(
    hir: &Hir,
    function_name: &str,
    block: &Block,
    value_types: &mut HashMap<String, String>,
) -> HirBlock {
    HirBlock {
        statements: block
            .statements
            .iter()
            .map(|statement| lower_hir_stmt(hir, function_name, statement, value_types))
            .collect(),
        span: block.span.clone(),
    }
}

fn lower_hir_stmt(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    value_types: &mut HashMap<String, String>,
) -> HirStmt {
    match statement {
        Stmt::Let(stmt) => {
            let type_name = stmt
                .value
                .as_ref()
                .and_then(|value| infer_hir_expr_type(hir, value, value_types));
            let value = stmt
                .value
                .as_ref()
                .map(|value| lower_hir_expr(hir, function_name, value, value_types));
            if let Some(type_name) = &type_name {
                value_types.insert(stmt.name.clone(), type_name.clone());
            }
            HirStmt::Let {
                kind: hir_binding_kind(stmt.kind),
                name: stmt.name.clone(),
                value,
                type_name,
                span: stmt.span.clone(),
            }
        }
        Stmt::Return(stmt) => {
            let proof = stmt
                .value
                .as_ref()
                .map_or(HirReturnProof::NoValue, |value| {
                    classify_return_expr(hir, value)
                });
            HirStmt::Return {
                value: stmt
                    .value
                    .as_ref()
                    .map(|value| lower_hir_expr(hir, function_name, value, value_types)),
                proof,
                span: stmt.span.clone(),
            }
        }
        Stmt::With(stmt) => {
            let resource_type = infer_hir_expr_type(hir, &stmt.resource, value_types);
            let mut body_types = value_types.clone();
            if let Some(resource_type) = &resource_type {
                body_types.insert(stmt.binding.clone(), resource_type.clone());
            }
            HirStmt::With {
                resource: lower_hir_expr(hir, function_name, &stmt.resource, value_types),
                binding: stmt.binding.clone(),
                body: lower_hir_block(hir, function_name, &stmt.body, &mut body_types),
                span: stmt.span.clone(),
            }
        }
        Stmt::If(stmt) => HirStmt::If {
            condition: lower_hir_expr(hir, function_name, &stmt.condition, value_types),
            then_body: {
                let mut then_types = value_types.clone();
                lower_hir_block(hir, function_name, &stmt.then_body, &mut then_types)
            },
            else_body: stmt.else_body.as_ref().map(|else_body| {
                let mut else_types = value_types.clone();
                lower_hir_block(hir, function_name, else_body, &mut else_types)
            }),
            span: stmt.span.clone(),
        },
        Stmt::Loop(stmt) => HirStmt::Loop {
            condition: stmt
                .condition
                .as_ref()
                .map(|condition| lower_hir_expr(hir, function_name, condition, value_types)),
            body: {
                let mut body_types = value_types.clone();
                lower_hir_block(hir, function_name, &stmt.body, &mut body_types)
            },
            span: stmt.span.clone(),
        },
        Stmt::Match(stmt) => {
            let value_type = infer_hir_expr_type(hir, &stmt.value, value_types);
            let value = lower_hir_expr(hir, function_name, &stmt.value, value_types);
            let arms = stmt
                .arms
                .iter()
                .map(|arm| {
                    let mut arm_types = value_types.clone();
                    if let Some((binding, type_name)) =
                        match_pattern_binding_type(&arm.pattern, value_type.as_deref())
                    {
                        arm_types.insert(binding, type_name);
                    }
                    HirMatchArm {
                        pattern: arm.pattern.clone(),
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirStmt::Match {
                value,
                arms,
                span: stmt.span.clone(),
            }
        }
        Stmt::Expr(expr) => HirStmt::Expr(lower_hir_expr(hir, function_name, expr, value_types)),
        Stmt::Break(span) => HirStmt::Break(span.clone()),
        Stmt::Continue(span) => HirStmt::Continue(span.clone()),
        Stmt::Unknown(span) => HirStmt::Unknown(span.clone()),
    }
}

fn lower_hir_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    value_types: &HashMap<String, String>,
) -> HirExpr {
    match expr {
        Expr::Ident(name, span) => HirExpr::Ident {
            name: name.clone(),
            type_name: value_types.get(name).cloned(),
            span: span.clone(),
        },
        Expr::Number(value, span) => HirExpr::Number {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::String(value, span) => HirExpr::String {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => HirExpr::Binary {
            op: *op,
            left: Box::new(lower_hir_expr(hir, function_name, left, value_types)),
            right: Box::new(lower_hir_expr(hir, function_name, right, value_types)),
            span: span.clone(),
        },
        Expr::Field { base, name, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types);
            let field = base_type
                .as_deref()
                .and_then(|type_name| hir.type_info(type_name))
                .and_then(|type_info| type_info.fields.get(name));
            HirExpr::Field {
                base: Box::new(lower_hir_expr(hir, function_name, base, value_types)),
                name: name.clone(),
                access: HirFieldAccess {
                    function_name: function_name.to_string(),
                    name: name.clone(),
                    span: span.clone(),
                    base_type,
                    type_name: field.map(|field| field.type_name.clone()),
                    is_handle: field.is_some_and(|field| field.is_handle || field.is_weak),
                },
                span: span.clone(),
            }
        }
        Expr::Index { base, index, span } => HirExpr::Index {
            base: Box::new(lower_hir_expr(hir, function_name, base, value_types)),
            index: Box::new(lower_hir_expr(hir, function_name, index, value_types)),
            span: span.clone(),
        },
        Expr::Call { callee, args, span } => {
            let resolution = hir.resolve_call(callee);
            let events = retain_events_for_call(function_name, callee, args, span, &resolution);
            let type_name = infer_hir_expr_type(hir, expr, value_types);
            HirExpr::Call {
                callee: callee.clone(),
                args: args
                    .iter()
                    .map(|arg| HirCallArg {
                        name: arg.name.clone(),
                        value: lower_hir_expr(hir, function_name, &arg.value, value_types),
                        span: arg.span.clone(),
                    })
                    .collect(),
                type_name,
                resolution,
                events,
                span: span.clone(),
            }
        }
        Expr::Effect {
            effect,
            value,
            span,
        } => HirExpr::Effect {
            effect: param_effect_from_data_effect(*effect),
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            events: effect_events_for_expr(function_name, expr),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::Manage { value, span } => HirExpr::Manage {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            events: effect_events_for_expr(function_name, expr),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::Try { value, span } => HirExpr::Try {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::Closure { body, span } => {
            let mut closure_types = value_types.clone();
            HirExpr::Closure {
                body: lower_hir_block(hir, function_name, body, &mut closure_types),
                span: span.clone(),
            }
        }
        Expr::Unknown(span) => HirExpr::Unknown(span.clone()),
    }
}

fn effect_events_for_expr(function_name: &str, expr: &Expr) -> Vec<HirEffectEvent> {
    let event = match expr {
        Expr::Manage { value, span } => {
            let Some((binding_name, value_span)) = direct_ident(value) else {
                return Vec::new();
            };
            HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Manage,
                binding_name,
                span: span.clone(),
                value_span,
            }
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            let Some((binding_name, value_span)) = direct_ident(value) else {
                return Vec::new();
            };
            HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Take,
                binding_name,
                span: span.clone(),
                value_span,
            }
        }
        Expr::Effect { .. } => return Vec::new(),
        _ => return Vec::new(),
    };
    vec![event]
}

fn retain_events_for_call(
    function_name: &str,
    callee: &Callee,
    args: &[crate::syntax::ast::CallArg],
    call_span: &Span,
    resolution: &CallResolution,
) -> Vec<HirEffectEvent> {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return Vec::new();
    };
    if signature.retained_params.is_empty() {
        return Vec::new();
    }

    args.iter()
        .filter_map(|arg| {
            let name = arg.name.as_ref()?;
            if !signature.retained_params.contains(name) {
                return None;
            }
            let (binding_name, value_span) = direct_read_ident(&arg.value)?;
            Some(HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Retain {
                    callee: callee_display(callee),
                    param: name.clone(),
                },
                binding_name,
                span: call_span.clone(),
                value_span,
            })
        })
        .collect()
}

fn collect_body_facts_in_block(
    hir: &Hir,
    function_name: &str,
    block: &Block,
    value_types: &mut HashMap<String, String>,
    facts: &mut BodyFacts,
) {
    for statement in &block.statements {
        collect_body_facts_in_stmt(hir, function_name, statement, value_types, facts);
    }
}

fn collect_body_facts_in_stmt(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    value_types: &mut HashMap<String, String>,
    facts: &mut BodyFacts,
) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function_name.to_string()),
                    kind: if matches!(stmt.value, Some(Expr::Closure { .. })) {
                        HirFeatureUseKind::LocalClosure
                    } else {
                        HirFeatureUseKind::LocalLet
                    },
                    span: stmt.span.clone(),
                });
            }
            let type_name = stmt
                .value
                .as_ref()
                .and_then(|value| infer_hir_expr_type(hir, value, value_types));
            facts.bindings.push(HirBinding {
                function_name: function_name.to_string(),
                name: stmt.name.clone(),
                kind: hir_binding_kind(stmt.kind),
                effect: None,
                span: stmt.span.clone(),
                type_name: type_name.clone(),
            });
            if let Some(type_name) = type_name {
                value_types.insert(stmt.name.clone(), type_name);
            }
            if let Some(value) = &stmt.value {
                collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                facts.returns.push(HirReturn {
                    function_name: function_name.to_string(),
                    span: value.span().clone(),
                    proof: classify_return_expr(hir, value),
                });
                collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
            } else {
                facts.returns.push(HirReturn {
                    function_name: function_name.to_string(),
                    span: stmt.span.clone(),
                    proof: HirReturnProof::NoValue,
                });
            }
        }
        Stmt::With(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.resource, value_types, facts);
            let resource_type = infer_hir_expr_type(hir, &stmt.resource, value_types);
            let mut body_types = value_types.clone();
            if let Some(resource_type) = resource_type {
                body_types.insert(stmt.binding.clone(), resource_type);
            }
            collect_body_facts_in_block(hir, function_name, &stmt.body, &mut body_types, facts);
        }
        Stmt::If(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.condition, value_types, facts);
            collect_body_facts_in_block(hir, function_name, &stmt.then_body, value_types, facts);
            if let Some(else_body) = &stmt.else_body {
                collect_body_facts_in_block(hir, function_name, else_body, value_types, facts);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_body_facts_in_expr(hir, function_name, condition, value_types, facts);
            }
            collect_body_facts_in_block(hir, function_name, &stmt.body, value_types, facts);
        }
        Stmt::Match(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
            let value_type = infer_hir_expr_type(hir, &stmt.value, value_types);
            for arm in &stmt.arms {
                let mut arm_types = value_types.clone();
                if let Some((binding, type_name)) =
                    match_pattern_binding_type(&arm.pattern, value_type.as_deref())
                {
                    facts.bindings.push(HirBinding {
                        function_name: function_name.to_string(),
                        name: binding.clone(),
                        kind: HirBindingKind::ManagedLet,
                        effect: None,
                        span: arm.span.clone(),
                        type_name: Some(type_name.clone()),
                    });
                    arm_types.insert(binding, type_name);
                }
                collect_body_facts_in_block(hir, function_name, &arm.body, &mut arm_types, facts);
            }
        }
        Stmt::Expr(expr) => {
            collect_body_facts_in_expr(hir, function_name, expr, value_types, facts);
        }
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
    }
}

fn collect_body_facts_in_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    value_types: &mut HashMap<String, String>,
    facts: &mut BodyFacts,
) {
    match expr {
        Expr::Binary { left, right, .. } => {
            collect_body_facts_in_expr(hir, function_name, left, value_types, facts);
            collect_body_facts_in_expr(hir, function_name, right, value_types, facts);
        }
        Expr::Call { callee, args, span } => {
            let resolution = hir.resolve_call(callee);
            if is_resource_pool_callee(callee) {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function_name.to_string()),
                    kind: HirFeatureUseKind::ResourcePool,
                    span: span.clone(),
                });
            }
            facts.call_sites.push(HirCallSite {
                function_name: function_name.to_string(),
                callee: callee.clone(),
                span: span.clone(),
                resolution: resolution.clone(),
            });
            collect_retain_events(function_name, callee, args, span, &resolution, facts);
            for arg in args {
                collect_body_facts_in_expr(hir, function_name, &arg.value, value_types, facts);
            }
        }
        Expr::Manage { value, span } => {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function_name.to_string()),
                kind: HirFeatureUseKind::Manage,
                span: span.clone(),
            });
            if let Some((binding_name, value_span)) = direct_ident(value) {
                facts.effect_events.push(HirEffectEvent {
                    function_name: function_name.to_string(),
                    kind: HirEffectEventKind::Manage,
                    binding_name,
                    span: span.clone(),
                    value_span,
                });
            }
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function_name.to_string()),
                kind: HirFeatureUseKind::Take,
                span: span.clone(),
            });
            if let Some((binding_name, value_span)) = direct_ident(value) {
                facts.effect_events.push(HirEffectEvent {
                    function_name: function_name.to_string(),
                    kind: HirEffectEventKind::Take,
                    binding_name,
                    span: span.clone(),
                    value_span,
                });
            }
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Effect { value, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Try { value, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Field { base, name, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types);
            let field = base_type
                .as_deref()
                .and_then(|type_name| hir.type_info(type_name))
                .and_then(|type_info| type_info.fields.get(name));
            facts.field_accesses.push(HirFieldAccess {
                function_name: function_name.to_string(),
                name: name.clone(),
                span: span.clone(),
                base_type,
                type_name: field.map(|field| field.type_name.clone()),
                is_handle: field.is_some_and(|field| field.is_handle || field.is_weak),
            });
            collect_body_facts_in_expr(hir, function_name, base, value_types, facts);
        }
        Expr::Index { base, index, .. } => {
            collect_body_facts_in_expr(hir, function_name, base, value_types, facts);
            collect_body_facts_in_expr(hir, function_name, index, value_types, facts);
        }
        Expr::Closure { body, .. } => {
            collect_body_facts_in_block(hir, function_name, body, value_types, facts);
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn collect_retain_events(
    function_name: &str,
    callee: &Callee,
    args: &[crate::syntax::ast::CallArg],
    call_span: &Span,
    resolution: &CallResolution,
    facts: &mut BodyFacts,
) {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return;
    };
    if signature.retained_params.is_empty() {
        return;
    }

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        if !signature.retained_params.contains(name) {
            continue;
        }
        if let Some((binding_name, value_span)) = direct_read_ident(&arg.value) {
            facts.effect_events.push(HirEffectEvent {
                function_name: function_name.to_string(),
                kind: HirEffectEventKind::Retain {
                    callee: callee_display(callee),
                    param: name.clone(),
                },
                binding_name,
                span: call_span.clone(),
                value_span,
            });
        }
    }
}

fn is_resource_pool_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Name(name) if name == "ResourcePool")
        || matches!(callee, Callee::Qualified { namespace, .. } if type_root_name(namespace) == "ResourcePool")
}

fn direct_read_ident(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } => direct_ident(value),
        _ => None,
    }
}

fn direct_ident(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        _ => None,
    }
}

fn hir_binding_kind(kind: LetKind) -> HirBindingKind {
    match kind {
        LetKind::Managed => HirBindingKind::ManagedLet,
        LetKind::Local => HirBindingKind::LocalLet,
    }
}

fn infer_hir_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => value_types.get(name).cloned(),
        Expr::Binary { .. } => None,
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            infer_hir_expr_type(hir, value, value_types)
        }
        Expr::Try { value, .. } => {
            infer_hir_expr_type(hir, value, value_types).and_then(|ty| result_ok_type(&ty))
        }
        Expr::Call { callee, args, .. } => match hir.resolve_call(callee) {
            CallResolution::Resolved { signature, .. } => {
                infer_builtin_generic_return_type(callee, args, value_types)
                    .or(signature.return_type)
            }
            CallResolution::EnumVariant | CallResolution::Unknown => None,
        },
        Expr::Field { base, name, .. } => {
            let base_type = infer_hir_expr_type(hir, base, value_types)?;
            hir.type_info(&base_type)?
                .fields
                .get(name)
                .map(|field| field.type_name.clone())
        }
        Expr::Index { .. } => None,
        Expr::Closure { .. } | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => None,
    }
}

fn infer_builtin_generic_return_type(
    callee: &Callee,
    args: &[CallArg],
    value_types: &HashMap<String, String>,
) -> Option<String> {
    if is_resource_pool_new(callee) {
        return resource_pool_namespace_arg(callee)
            .map(|resource| format!("ResourcePool<{resource}>"));
    }
    if is_resource_pool_borrow(callee) {
        return resource_pool_borrow_type(callee, args, value_types);
    }
    None
}

fn resource_pool_borrow_type(
    callee: &Callee,
    args: &[CallArg],
    value_types: &HashMap<String, String>,
) -> Option<String> {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some("pool"))
        .and_then(|arg| resource_pool_arg_type(&arg.value, value_types))
        .or_else(|| resource_pool_namespace_arg(callee).map(str::to_string))
}

fn resource_pool_arg_type(expr: &Expr, value_types: &HashMap<String, String>) -> Option<String> {
    let type_name = match expr {
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            return resource_pool_arg_type(value, value_types);
        }
        Expr::Ident(name, _) => value_types.get(name)?,
        Expr::Call { callee, .. } => {
            return resource_pool_namespace_arg(callee).map(|resource| resource.to_string());
        }
        Expr::Field { .. }
        | Expr::Index { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Unknown(_) => return None,
    };
    resource_pool_type_arg(type_name).map(str::to_string)
}

fn is_resource_pool_new(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && name == "new")
}

fn is_resource_pool_borrow(callee: &Callee) -> bool {
    matches!(callee, Callee::Qualified { namespace, name } if type_root_name(namespace) == "ResourcePool" && name == "borrow")
}

fn resource_pool_namespace_arg(callee: &Callee) -> Option<&str> {
    match callee {
        Callee::Qualified { namespace, .. } => resource_pool_type_arg(namespace),
        Callee::Name(_) => None,
    }
}

fn resource_pool_type_arg(type_name: &str) -> Option<&str> {
    type_name
        .strip_prefix("ResourcePool<")
        .and_then(|rest| rest.strip_suffix('>'))
}

fn result_ok_type(type_name: &str) -> Option<String> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|rest| rest.strip_suffix('>'))?;
    split_top_level_type_args(inner)
        .into_iter()
        .next()
        .map(str::to_string)
}

fn match_pattern_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&str>,
) -> Option<(String, String)> {
    let MatchPattern::Variant {
        name,
        binding: Some(binding),
        ..
    } = pattern
    else {
        return None;
    };
    let value_type = value_type?;
    let inner = value_type
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'));
    if name == "Some" {
        return inner.map(|ty| (binding.clone(), ty.trim().to_string()));
    }
    let inner = value_type
        .strip_prefix("Result<")
        .and_then(|rest| rest.strip_suffix('>'));
    let args = inner.map(split_top_level_type_args)?;
    match name.as_str() {
        "Ok" => args
            .first()
            .map(|ty| (binding.clone(), ty.trim().to_string())),
        "Err" => args
            .get(1)
            .map(|ty| (binding.clone(), ty.trim().to_string())),
        _ => None,
    }
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if start < args.len() {
        parts.push(args[start..].trim());
    }
    parts
}

fn classify_return_expr(hir: &Hir, expr: &Expr) -> HirReturnProof {
    match expr {
        Expr::Ident(name, _) => HirReturnProof::Ident { name: name.clone() },
        Expr::Call { callee, args, .. } => {
            if matches!(callee_name(callee), "Ok" | "Some")
                && let Some(arg) = args.first()
            {
                return classify_return_expr(hir, &arg.value);
            }
            match hir.resolve_call(callee) {
                CallResolution::Resolved {
                    signature,
                    kind:
                        ResolvedCalleeKind::Constructor {
                            type_kind: HirTypeKind::Struct,
                        },
                } if signature.returns_fresh => HirReturnProof::StructConstructor,
                CallResolution::Resolved { signature, .. } if signature.returns_fresh => {
                    HirReturnProof::FreshCall
                }
                CallResolution::Resolved {
                    kind:
                        ResolvedCalleeKind::Constructor {
                            type_kind: HirTypeKind::Struct,
                        },
                    ..
                } => HirReturnProof::StructConstructor,
                CallResolution::Resolved { .. }
                | CallResolution::EnumVariant
                | CallResolution::Unknown => HirReturnProof::Unknown,
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            classify_return_expr(hir, value)
        }
        Expr::Field { .. }
        | Expr::Index { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Unknown(_) => HirReturnProof::Unknown,
    }
}

fn function_kind(signature: &FunctionSig) -> ResolvedCalleeKind {
    if signature.is_builtin {
        ResolvedCalleeKind::BuiltinFunction
    } else {
        ResolvedCalleeKind::UserFunction
    }
}

fn is_enum_variant_call(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some" | "None" | "Result" | "Option")
}

fn callee_name(callee: &Callee) -> &str {
    match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => name,
    }
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
    }
}

fn function_sig_from_decl(function: &FunctionDecl, is_builtin: bool) -> FunctionSig {
    let (namespace, name) = split_function_name(&function.name);
    FunctionSig {
        namespace,
        name,
        is_async: function.is_async,
        params: function.params.iter().map(param_sig_from_decl).collect(),
        return_type: function.return_ty.as_ref().map(type_ref_name),
        returns_fresh: function.returns_fresh,
        effects: function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Name(name) => Some(name.clone()),
                EffectDecl::Retains(_) => None,
            })
            .collect(),
        retained_params: function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Retains(param) => Some(param.clone()),
                EffectDecl::Name(_) => None,
            })
            .collect(),
        is_builtin,
    }
}

fn split_function_name(name: &str) -> (Option<String>, String) {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        (Some(namespace.to_string()), name.to_string())
    } else {
        (None, name.to_string())
    }
}

fn type_ref_name(ty: &TypeRef) -> String {
    let name = if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    if ty.is_noescape {
        if ty.name == "Fn" && ty.args.is_empty() {
            return "noescape Fn()".to_string();
        }
        format!("noescape {name}")
    } else {
        name
    }
}

fn type_root_name(type_name: &str) -> &str {
    type_name
        .split_once('<')
        .map_or(type_name, |(root, _)| root)
}

fn record_duplicate_symbol(
    duplicates: &mut Vec<DuplicateSymbol>,
    symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
    kind: DuplicateSymbolKind,
    name: &str,
    span: &Span,
) {
    if let Some((first_kind, first_span)) = symbols.get(name) {
        duplicates.push(DuplicateSymbol {
            kind: duplicate_symbol_kind(*first_kind, kind),
            name: name.to_string(),
            first_span: first_span.clone(),
            duplicate_span: span.clone(),
        });
        return;
    }

    symbols.insert(name.to_string(), (kind, span.clone()));
}

fn record_duplicate_fields(duplicates: &mut Vec<DuplicateSymbol>, type_decl: &TypeDecl) {
    let mut fields = HashMap::new();
    for field in &type_decl.fields {
        record_duplicate_symbol(
            duplicates,
            &mut fields,
            DuplicateSymbolKind::Field,
            &format!("{}.{}", type_decl.name, field.name),
            &field.span,
        );
    }
}

fn duplicate_symbol_kind(
    first: DuplicateSymbolKind,
    duplicate: DuplicateSymbolKind,
) -> DuplicateSymbolKind {
    match (first, duplicate) {
        (DuplicateSymbolKind::Function, DuplicateSymbolKind::Function) => {
            DuplicateSymbolKind::Function
        }
        (DuplicateSymbolKind::Type, DuplicateSymbolKind::Type) => DuplicateSymbolKind::Type,
        (DuplicateSymbolKind::Field, DuplicateSymbolKind::Field) => DuplicateSymbolKind::Field,
        _ => DuplicateSymbolKind::Constructor,
    }
}

fn param_sig_from_decl(param: &Param) -> ParamSig {
    ParamSig {
        name: param.name.clone(),
        effect: param.effect.map(param_effect_from_data_effect),
        type_name: type_ref_name(&param.ty),
    }
}

fn param_effect_from_data_effect(effect: DataEffect) -> ParamEffect {
    match effect {
        DataEffect::Read => ParamEffect::Read,
        DataEffect::Mut => ParamEffect::Mut,
        DataEffect::Take => ParamEffect::Take,
    }
}

fn type_info_from_decl(type_decl: &TypeDecl) -> TypeInfo {
    TypeInfo {
        name: type_decl.name.clone(),
        kind: type_kind_from_decl(type_decl.kind),
        fields: type_decl
            .fields
            .iter()
            .map(|field| (field.name.clone(), field_info_from_decl(field)))
            .collect(),
    }
}

fn type_kind_from_decl(kind: TypeKind) -> HirTypeKind {
    match kind {
        TypeKind::Class => HirTypeKind::Class,
        TypeKind::Struct => HirTypeKind::Struct,
        TypeKind::Resource => HirTypeKind::Resource,
    }
}

fn field_info_from_decl(field: &FieldDecl) -> FieldInfo {
    FieldInfo {
        name: field.name.clone(),
        type_name: type_ref_name(&field.ty),
        is_handle: field.is_handle,
        is_weak: field.is_weak,
    }
}

fn constructor_sig_from_type(type_info: &TypeInfo, is_builtin: bool) -> FunctionSig {
    let mut fields: Vec<&FieldInfo> = type_info.fields.values().collect();
    fields.sort_by(|left, right| left.name.cmp(&right.name));

    FunctionSig {
        namespace: None,
        name: type_info.name.clone(),
        is_async: false,
        params: fields
            .into_iter()
            .map(|field| ParamSig {
                name: field.name.clone(),
                effect: None,
                type_name: field.type_name.clone(),
            })
            .collect(),
        return_type: Some(type_info.name.clone()),
        returns_fresh: type_info.kind == HirTypeKind::Struct,
        effects: Vec::new(),
        retained_params: HashSet::new(),
        is_builtin,
    }
}

fn qualified_key(namespace: &str, name: &str) -> String {
    format!("{namespace}.{name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse_source;

    #[test]
    fn collects_type_kinds_and_handle_fields() {
        let source = r#"
features: local

class User {
    name: String
}

resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Session {
    user: handle User
    parent: weak User
    file_name: String
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);

        assert_eq!(hir.type_kind("User"), Some(HirTypeKind::Class));
        assert_eq!(hir.type_kind("File"), Some(HirTypeKind::Resource));
        assert_eq!(hir.type_kind("Session"), Some(HirTypeKind::Struct));

        let user_field = hir.fields_named("user").next().expect("user field exists");
        assert_eq!(user_field.type_name, "User");
        assert!(user_field.is_handle);
        assert!(!user_field.is_weak);
        let parent_field = hir
            .fields_named("parent")
            .next()
            .expect("parent field exists");
        assert_eq!(parent_field.type_name, "User");
        assert!(!parent_field.is_handle);
        assert!(parent_field.is_weak);
        let session = hir.type_info("Session").expect("session type exists");
        assert!(session.fields["user"].is_handle);
        assert!(session.fields["parent"].is_weak);
        assert!(!session.fields["file_name"].is_handle);
        assert!(hir.is_handle_field_name("user"));
        assert!(hir.is_handle_field_name("parent"));
        assert!(!hir.is_handle_field_name("file_name"));
    }

    #[test]
    fn keeps_builtin_and_user_function_signatures() {
        let source = r#"

fn cache_put(cache: mut Cache, value: read Image) -> Unit
    effects(retains(value))
{
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);

        assert!(hir.resolve_function(Some("Image"), "resize").is_some());

        let signature = hir
            .resolve_function(None, "cache_put")
            .expect("user signature exists");
        assert!(signature.retained_params.contains("value"));
        assert_eq!(signature.params[0].effect, Some(ParamEffect::Mut));
        assert_eq!(signature.params[1].effect, Some(ParamEffect::Read));
        assert_eq!(signature.return_type.as_deref(), Some("Unit"));

        let load = hir
            .resolve_function(Some("Image"), "load")
            .expect("builtin signature exists");
        assert_eq!(
            load.return_type.as_deref(),
            Some("Result<Image, ImageError>")
        );
    }

    #[test]
    fn records_duplicate_callable_symbols() {
        let source = r#"

struct Image {
    pixels: Buffer
}

fn Image(path: read Path) -> Image {
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let duplicate = hir
            .duplicate_symbols()
            .first()
            .expect("constructor/function duplicate is recorded");

        assert_eq!(duplicate.kind, DuplicateSymbolKind::Constructor);
        assert_eq!(duplicate.name, "Image");
        assert_eq!(duplicate.first_span.line, 3);
        assert_eq!(duplicate.duplicate_span.line, 7);
    }

    #[test]
    fn records_duplicate_fields() {
        let source = r#"

struct Response {
    status: Int
    status: String
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let duplicate = hir
            .duplicate_symbols()
            .first()
            .expect("duplicate field is recorded");

        assert_eq!(duplicate.kind, DuplicateSymbolKind::Field);
        assert_eq!(duplicate.name, "Response.status");
        assert_eq!(duplicate.first_span.line, 4);
        assert_eq!(duplicate.duplicate_span.line, 5);
    }

    #[test]
    fn resolves_body_call_sites() {
        let source = r#"

struct Response {
    status: Int
    body: String
}

fn render(body: read String) -> Result<fresh Response, HttpError> {
    let response = Response(status: 200, body: read body)
    Log.write(message: read body)
    Missing.call(value: read body)
    return response
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let sites = &hir.call_sites;

        assert_eq!(sites.len(), 3);
        assert!(matches!(
            sites[0].resolution,
            CallResolution::Resolved {
                kind: ResolvedCalleeKind::Constructor {
                    type_kind: HirTypeKind::Struct
                },
                ..
            }
        ));
        assert!(matches!(
            sites[1].resolution,
            CallResolution::Resolved {
                kind: ResolvedCalleeKind::BuiltinFunction,
                ..
            }
        ));
        assert!(matches!(sites[2].resolution, CallResolution::Unknown));

        let bindings = &hir
            .function_body("render")
            .expect("render body exists")
            .bindings;
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].kind, HirBindingKind::Param);
        assert_eq!(bindings[0].name, "body");
        assert_eq!(bindings[0].type_name.as_deref(), Some("String"));
        assert_eq!(bindings[1].kind, HirBindingKind::ManagedLet);
        assert_eq!(bindings[1].name, "response");
        assert_eq!(bindings[1].type_name.as_deref(), Some("Response"));

        let returns = &hir.returns;
        assert_eq!(returns.len(), 1);
        assert_eq!(returns[0].function_name, "render");
        assert!(matches!(
            returns[0].proof,
            HirReturnProof::Ident { ref name } if name == "response"
        ));

        let body = hir.function_body("render").expect("function body exists");
        assert_eq!(body.function_name, "render");
        assert_eq!(body.bindings.len(), 2);
        assert_eq!(body.call_sites.len(), 3);
        assert_eq!(body.effect_events.len(), 0);
        assert_eq!(body.returns.len(), 1);
        assert!(matches!(
            body.block
                .as_ref()
                .expect("resolved body block exists")
                .statements
                .first(),
            Some(HirStmt::Let {
                kind: HirBindingKind::ManagedLet,
                type_name: Some(type_name),
                ..
            }) if type_name == "Response"
        ));
    }

    #[test]
    fn records_local_binding_facts() {
        let source = r#"
features: local

fn load(path: read Path) -> Unit {
    local image = Image.load(path: read path)?
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let bindings = &hir
            .function_body("load")
            .expect("load body exists")
            .bindings;

        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[1].kind, HirBindingKind::LocalLet);
        assert_eq!(bindings[1].name, "image");
        assert_eq!(bindings[1].type_name.as_deref(), Some("Image"));
        assert!(matches!(
            hir.function_body("load")
                .and_then(|body| body.block.as_ref())
                .and_then(|block| block.statements.first()),
            Some(HirStmt::Let {
                kind: HirBindingKind::LocalLet,
                type_name: Some(type_name),
                ..
            }) if type_name == "Image"
        ));
    }

    #[test]
    fn propagates_resource_pool_generic_lease_types() {
        let source = r#"
features: local

resource DbConnection {
    fd: Int

    drop {
        Db.close(fd: fd)
    }
}

fn run(pool: mut ResourcePool<DbConnection>) -> Unit {
    with ResourcePool.borrow(pool: mut pool) as conn {
        DbConnection.query(conn: mut conn)
    }
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let body = hir.function_body("run").expect("run body exists");

        assert_eq!(
            body.bindings[0].type_name.as_deref(),
            Some("ResourcePool<DbConnection>")
        );
        let block = body.block.as_ref().expect("resolved body block exists");
        let HirStmt::With {
            resource,
            body: with_body,
            ..
        } = &block.statements[0]
        else {
            panic!("expected with statement");
        };
        assert!(matches!(
            resource,
            HirExpr::Call {
                type_name: Some(type_name),
                ..
            } if type_name == "DbConnection"
        ));
        assert!(matches!(
            &with_body.statements[0],
            HirStmt::Expr(HirExpr::Call { args, .. })
                if matches!(
                    &args[0].value,
                    HirExpr::Effect {
                        value,
                        type_name: Some(type_name),
                        ..
                    } if type_name == "DbConnection"
                        && matches!(
                            value.as_ref(),
                            HirExpr::Ident {
                                name,
                                type_name: Some(ident_type),
                                ..
                            } if name == "conn" && ident_type == "DbConnection"
                        )
                )
        ));
    }

    #[test]
    fn records_field_access_facts() {
        let source = r#"
features: local

class Rules {
}

struct Config {
    rules: handle Rules
}

fn take_rules(config: mut Config) -> Unit {
    List.consume(list: take config.rules)
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let field = hir
            .field_accesses
            .first()
            .expect("field access is recorded");

        assert_eq!(field.function_name, "take_rules");
        assert_eq!(field.name, "rules");
        assert_eq!(field.base_type.as_deref(), Some("Config"));
        assert_eq!(field.type_name.as_deref(), Some("Rules"));
        assert!(field.is_handle);
        assert!(
            hir.function_body("take_rules")
                .expect("body exists")
                .field_accesses
                .iter()
                .any(|access| access.name == "rules" && access.is_handle)
        );
    }

    #[test]
    fn records_effect_events() {
        let source = r#"
features: local

fn publish(cache: mut ImageCache, path: read Path) -> Unit {
    local image = Image.load(path: read path)
    let shared = manage image
    ImageCache.store(cache: mut cache, image: read shared)
    Buffer.consume(buffer: take image)
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);

        assert_eq!(hir.effect_events.len(), 3);
        assert!(matches!(
            hir.effect_events[0].kind,
            HirEffectEventKind::Manage
        ));
        assert_eq!(hir.effect_events[0].binding_name, "image");
        assert!(matches!(
            hir.effect_events[1].kind,
            HirEffectEventKind::Retain { .. }
        ));
        assert_eq!(hir.effect_events[1].binding_name, "shared");
        assert!(matches!(
            hir.effect_events[2].kind,
            HirEffectEventKind::Take
        ));
        assert_eq!(hir.effect_events[2].binding_name, "image");
        assert_eq!(
            hir.function_body("publish")
                .expect("publish body exists")
                .effect_events
                .len(),
            3
        );
    }

    #[test]
    fn lowers_resolved_statement_expression_tree_for_function_body() {
        let source = r#"
features: local

class Rules {
}

struct Config {
    rules: handle Rules
}

fn update(cache: mut ImageCache, config: mut Config, path: read Path) -> Unit {
    local image = Image.load(path: read path)?
    ImageCache.store(cache: mut cache, image: read image)
    List.consume(list: take config.rules)
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let body = hir.function_body("update").expect("body exists");
        let block = body.block.as_ref().expect("resolved HIR block exists");

        assert_eq!(block.statements.len(), 3);
        let HirStmt::Let {
            kind: HirBindingKind::LocalLet,
            name,
            value: Some(HirExpr::Try { type_name, .. }),
            type_name: Some(binding_type),
            ..
        } = &block.statements[0]
        else {
            panic!("first statement should be a typed local call binding");
        };
        assert_eq!(name, "image");
        assert_eq!(type_name.as_deref(), Some("Image"));
        assert_eq!(binding_type, "Image");

        let HirStmt::Expr(HirExpr::Call {
            resolution, events, ..
        }) = &block.statements[1]
        else {
            panic!("second statement should be a resolved retaining call");
        };
        assert!(matches!(
            resolution,
            CallResolution::Resolved {
                kind: ResolvedCalleeKind::BuiltinFunction,
                ..
            }
        ));
        assert!(matches!(events[0].kind, HirEffectEventKind::Retain { .. }));
        assert_eq!(events[0].binding_name, "image");

        let HirStmt::Expr(HirExpr::Call { args, .. }) = &block.statements[2] else {
            panic!("third statement should be a call");
        };
        let HirExpr::Effect {
            effect: ParamEffect::Take,
            value,
            events,
            ..
        } = &args[0].value
        else {
            panic!("call argument should be a take expression");
        };
        assert!(events.is_empty());
        let HirExpr::Field { access, .. } = value.as_ref() else {
            panic!("take value should be a field access");
        };
        assert_eq!(access.base_type.as_deref(), Some("Config"));
        assert_eq!(access.type_name.as_deref(), Some("Rules"));
        assert!(access.is_handle);
    }

    #[test]
    fn classifies_fresh_return_facts() {
        let source = r#"

struct Response {
    status: Int
}

fn make_response() -> fresh Response {
    return Response(status: 200)
}
"#;

        let program = parse_source("test.rss", source);
        let hir = Hir::from_syntax(&program);
        let return_fact = hir.returns.first().expect("return fact exists");

        assert_eq!(return_fact.function_name, "make_response");
        assert!(matches!(
            return_fact.proof,
            HirReturnProof::StructConstructor
        ));
    }
}
