use super::*;

impl Hir {
    pub fn from_syntax(program: &SyntaxProgram) -> Self {
        Self::from_syntax_with_interfaces(program, &[])
    }

    pub fn from_syntax_with_standard_package_interfaces(program: &SyntaxProgram) -> Self {
        Self::from_syntax_with_interfaces_options(program, &[], true)
    }

    pub fn from_syntax_with_interfaces(
        program: &SyntaxProgram,
        interfaces: &[SyntaxProgram],
    ) -> Self {
        Self::from_syntax_with_interfaces_options(program, interfaces, false)
    }

    pub(crate) fn from_syntax_with_prepared_interfaces(
        program: &SyntaxProgram,
        builtin_interfaces: &[SyntaxProgram],
        interfaces: &[SyntaxProgram],
    ) -> Self {
        let mut hir = Self::default();
        for interface in builtin_interfaces {
            hir.insert_builtin_interface(interface);
        }
        let mut type_symbols: HashMap<String, (DuplicateSymbolKind, Span)> = HashMap::new();
        let mut callable_symbols: HashMap<String, (DuplicateSymbolKind, Span)> = HashMap::new();
        for interface in interfaces {
            hir.extend_protocol_impls(&interface.protocol_impls, false);
            hir.collect_item_signatures(interface, &mut type_symbols, &mut callable_symbols);
        }
        hir.extend_protocol_impls(&program.protocol_impls, true);
        hir.collect_item_signatures(program, &mut type_symbols, &mut callable_symbols);
        hir.normalize_class_typed_handle_fields();
        hir.collect_const_values(program);
        hir.collect_resource_drop_bodies(program);
        hir.collect_body_facts(program);
        hir
    }

    /// Record top-level `const` initializers so references can be inlined during
    /// lowering (the register VM has no const/global slots). Initializers are
    /// literals (the checker enforces this), so inlining is exact.
    fn collect_const_values(&mut self, program: &SyntaxProgram) {
        for item in &program.items {
            if let Item::Const(decl) = item {
                self.const_values
                    .insert(decl.name.clone(), decl.value.clone());
            }
        }
    }

    fn from_syntax_with_interfaces_options(
        program: &SyntaxProgram,
        interfaces: &[SyntaxProgram],
        include_standard_package_interfaces: bool,
    ) -> Self {
        let mut builtin_interface_programs = builtin_interfaces()
            .map(|(file, source)| parse_source(file, source))
            .collect::<Vec<_>>();
        if include_standard_package_interfaces {
            builtin_interface_programs.extend(
                standard_package_interfaces().map(|(file, source)| parse_source(file, source)),
            );
        }

        Self::from_syntax_with_prepared_interfaces(program, &builtin_interface_programs, interfaces)
    }

    fn extend_protocol_impls(&mut self, impls: &[ProtocolImpl], is_current_program: bool) {
        self.protocol_impls
            .extend(impls.iter().map(|protocol_impl| HirProtocolImpl {
                protocol: protocol_impl.protocol.clone(),
                type_name: protocol_impl.type_name.clone(),
                mappings: protocol_impl.mappings.clone(),
                is_current_program,
            }));
    }

    /// A field whose declared type is a `class` is always a handle field
    /// (spec §6.5), matching the rule the Rust lowering applies via type kinds.
    /// The parser only sets `is_handle` from the explicit `handle`/`weak`
    /// keyword, so class-typed fields are promoted here before conflict-root and
    /// retention analysis read field handle-ness from the type table.
    fn normalize_class_typed_handle_fields(&mut self) {
        let class_types: HashSet<String> = self
            .types
            .iter()
            .filter(|(_, info)| info.kind == HirTypeKind::Class)
            .map(|(name, _)| name.clone())
            .collect();
        for info in self.types.values_mut() {
            for field in info.fields.values_mut() {
                if !field.is_handle
                    && !field.is_weak
                    && class_types.contains(type_root_name(&field.type_name))
                {
                    field.is_handle = true;
                }
            }
            for field in &mut info.fields_ordered {
                if !field.is_handle
                    && !field.is_weak
                    && class_types.contains(type_root_name(&field.type_name))
                {
                    field.is_handle = true;
                }
            }
        }
        self.fields_by_name.clear();
        for info in self.types.values() {
            for field in info.fields.values() {
                self.fields_by_name
                    .entry(field.name.clone())
                    .or_default()
                    .push(field.clone());
            }
        }
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
                    // Mirror rust_lower's derive emission: an omitted derive list defaults to
                    // Debug+Clone for non-resource types, so those are cloneable too.
                    if type_decl.derives.iter().any(|d| d == "Clone")
                        || (type_decl.derives.is_empty() && type_decl.kind != TypeKind::Resource)
                    {
                        self.clone_types.insert(type_decl.name.clone());
                    }
                    self.insert_type(type_info_from_decl(type_decl));
                }
                Item::SumType(sum) => {
                    record_duplicate_symbol(
                        &mut self.duplicate_symbols,
                        type_symbols,
                        DuplicateSymbolKind::Type,
                        &sum.name,
                        &sum.span,
                    );
                    let type_info = TypeInfo {
                        name: sum.name.clone(),
                        kind: HirTypeKind::Sum,
                        type_params: sum
                            .type_params
                            .iter()
                            .map(|p| p.name.clone())
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                        fields_ordered: Vec::new(),
                        fields: HashMap::new(),
                    };
                    // Sums are never resources; an omitted derive list defaults to Debug+Clone.
                    if sum.derives.is_empty() || sum.derives.iter().any(|d| d == "Clone") {
                        self.clone_types.insert(sum.name.clone());
                    }
                    self.insert_type(type_info);
                    for variant in &sum.variants {
                        self.sum_variant_types
                            .insert(variant.name.clone(), sum.name.clone());
                        self.sum_variant_fields.insert(
                            variant.name.clone(),
                            variant.fields.iter().map(field_info_from_decl).collect(),
                        );
                    }
                }
                Item::TypeAlias(_) | Item::Const(_) | Item::Module(_) | Item::Use(_) => {}
            }
        }
    }

    fn collect_resource_drop_bodies(&mut self, program: &SyntaxProgram) {
        for item in &program.items {
            let Item::Type(type_decl) = item else {
                continue;
            };
            if type_decl.kind != TypeKind::Resource {
                continue;
            }
            let Some(drop_body) = &type_decl.drop_body else {
                continue;
            };
            let mut value_types = type_decl
                .fields
                .iter()
                .map(|field| (field.name.clone(), type_ref_name(&field.ty)))
                .collect::<HashMap<_, _>>();
            let body = lower_hir_block(
                self,
                &format!("{}.drop", type_decl.name),
                drop_body,
                &mut value_types,
            );
            self.resource_drop_bodies
                .insert(type_decl.name.clone(), body);
        }
    }

    /// Concrete impl targets for a protocol method: `(implementing type name,
    /// target function name)` for every `impl Protocol for Type` that maps
    /// `method`. Used by the reg-VM to dynamically dispatch a `Protocol.method`
    /// call by the receiver's runtime type (capability objects + generic bounds),
    /// mirroring the compiled backend's closed-world enum dispatch.
    pub(crate) fn protocol_method_targets(
        &self,
        protocol: &str,
        method: &str,
    ) -> Vec<(String, String)> {
        let protocol = type_root_name(protocol);
        let method = type_root_name(method);
        self.protocol_impls
            .iter()
            .filter(|imp| imp.protocol == protocol)
            .filter_map(|imp| {
                imp.mappings
                    .iter()
                    .find(|mapping| mapping.method == method)
                    .map(|mapping| (imp.type_name.clone(), mapping.target.clone()))
            })
            .collect()
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

    pub fn types(&self) -> impl Iterator<Item = &TypeInfo> {
        self.types.values()
    }

    pub fn type_kind(&self, name: &str) -> Option<HirTypeKind> {
        self.type_info(name).map(|info| info.kind)
    }

    pub fn sum_type_for_variant(&self, variant_name: &str) -> Option<&str> {
        self.sum_variant_types.get(variant_name).map(String::as_str)
    }

    pub fn sum_variant_fields(&self, variant_name: &str) -> Option<&[FieldInfo]> {
        self.sum_variant_fields.get(variant_name).map(Vec::as_slice)
    }

    #[cfg(test)]
    pub(super) fn fields_named(&self, field_name: &str) -> impl Iterator<Item = &FieldInfo> {
        self.fields_by_name
            .get(field_name)
            .into_iter()
            .flat_map(|fields| fields.iter())
    }

    #[cfg(test)]
    pub(super) fn is_handle_field_name(&self, field_name: &str) -> bool {
        self.fields_named(field_name)
            .any(|field| field.is_handle || field.is_weak)
    }

    pub fn duplicate_symbols(&self) -> &[DuplicateSymbol] {
        &self.duplicate_symbols
    }

    pub fn function_body(&self, function_name: &str) -> Option<&HirFunctionBody> {
        self.function_bodies.get(function_name)
    }

    pub fn function_bodies(&self) -> impl Iterator<Item = (&str, &HirFunctionBody)> {
        self.function_bodies
            .iter()
            .map(|(name, body)| (name.as_str(), body))
    }

    pub fn resource_drop_bodies(&self) -> impl Iterator<Item = (&str, &HirBlock)> {
        self.resource_drop_bodies
            .iter()
            .map(|(type_name, body)| (type_name.as_str(), body))
    }

    pub fn feature_uses(&self) -> &[HirFeatureUse] {
        &self.feature_uses
    }

    pub fn call_sites(&self) -> &[HirCallSite] {
        &self.call_sites
    }

    pub fn resolve_call(&self, callee: &Callee) -> CallResolution {
        let call_name = callee_name(callee);
        // Builtin variants (Ok/Err/Some/None) and user-declared sum variants are both
        // constructor calls. The reg-VM lowerer already builds user payload variants via
        // `sum_variant_fields`; recognizing them here lets construction resolve instead of
        // being reported as an unknown callee.
        if is_enum_variant_call(call_name) || self.sum_type_for_variant(call_name).is_some() {
            return CallResolution::EnumVariant;
        }

        let signature = match callee {
            Callee::Name(name) => self.resolve_function(None, type_root_name(name)),
            Callee::Qualified { namespace, name } => {
                self.resolve_function(Some(namespace), type_root_name(name))
            }
            Callee::ReceiverCall { .. } => {
                // ReceiverCall requires type context; use resolve_receiver_call instead
                return CallResolution::Unknown;
            }
        };
        let Some(signature) = signature else {
            return CallResolution::Unknown;
        };
        let kind = match callee {
            Callee::Name(name) => self.type_kind(name).map_or_else(
                || function_kind(signature),
                |type_kind| ResolvedCalleeKind::Constructor { type_kind },
            ),
            Callee::Qualified { .. } | Callee::ReceiverCall { .. } => function_kind(signature),
        };

        CallResolution::Resolved {
            signature: signature.clone(),
            kind,
        }
    }

    /// Resolve a receiver-call shorthand given the receiver's resolved type.
    /// Returns the resolution and the namespace used (type or protocol name).
    /// `value_types` is used to look up protocol bounds for generic type params
    /// (stored with key `__protocol_bound__<TypeParam>`).
    pub fn resolve_receiver_call(
        &self,
        receiver_type: &str,
        method: &str,
        value_types: &HashMap<String, String>,
    ) -> (CallResolution, Option<String>) {
        let candidates = self.receiver_call_candidates(receiver_type, method, value_types);
        if candidates.is_empty() {
            // A declared (user) type only gets the synthesized `.clone()` if it derives `Clone`;
            // otherwise leave the call unresolved (RS0206) instead of emitting an `.clone()` that
            // Rust would reject (E0599). Non-user receivers keep their existing resolution.
            let clone_allowed = {
                let root = type_root_name(receiver_type);
                !self.types.contains_key(root) || self.clone_types.contains(root)
            };
            if method == "clone" && clone_allowed {
                // Every value supports an explicit `.clone()` returning a fresh copy of the
                // receiver's type. The runtime already clones implicitly (e.g. `read` args stored
                // into a collection); this exposes that as a callable for any `derives(Clone)` type.
                return (
                    CallResolution::Resolved {
                        signature: FunctionSig {
                            namespace: Some(type_root_name(receiver_type).to_string()),
                            name: "clone".to_string(),
                            is_public: true,
                            is_async: false,
                            is_native: false,
                            type_params: Box::from([]),
                            type_param_bounds: Vec::new(),
                            params: vec![ParamSig {
                                name: "self".to_string(),
                                effect: Some(ParamEffect::Read),
                                type_name: receiver_type.to_string(),
                                default: None,
                            }],
                            return_type: Some(receiver_type.to_string()),
                            returns_fresh: true,
                            effects: Vec::new(),
                            retained_params: HashSet::new(),
                            is_builtin: true,
                        },
                        kind: ResolvedCalleeKind::BuiltinFunction,
                    },
                    Some(type_root_name(receiver_type).to_string()),
                );
            }
            return (CallResolution::Unknown, None);
        }
        if candidates.len() > 1 {
            return (
                CallResolution::Ambiguous {
                    candidates: candidates
                        .iter()
                        .map(|(namespace, _)| format!("{namespace}.{method}"))
                        .collect(),
                },
                None,
            );
        }
        let (namespace, sig) = &candidates[0];
        (
            CallResolution::Resolved {
                signature: sig.clone(),
                kind: function_kind(sig),
            },
            Some(namespace.clone()),
        )
    }

    fn receiver_call_candidates(
        &self,
        receiver_type: &str,
        method: &str,
        value_types: &HashMap<String, String>,
    ) -> Vec<(String, FunctionSig)> {
        let receiver_root = type_root_name(receiver_type);
        let mut candidates = Vec::new();

        if let Some(sig) = self.resolve_function(Some(receiver_root), method) {
            candidates.push((receiver_root.to_string(), sig.clone()));
        }
        if let Some(namespace) = receiver_facade_namespace(receiver_root, method)
            && let Some(sig) = self.resolve_function(Some(namespace), method)
        {
            candidates.push((namespace.to_string(), sig.clone()));
        }

        let bound_key = format!("__protocol_bound__{receiver_root}");
        if let Some(protocol) = value_types.get(&bound_key) {
            if let Some(sig) = self.resolve_function(Some(protocol), method) {
                candidates.push((protocol.clone(), sig.clone()));
            }
        }
        if let Some(protocol) = capability_protocol(receiver_type)
            && let Some(sig) = self.resolve_function(Some(protocol), method)
        {
            candidates.push((protocol.to_string(), sig.clone()));
        }

        for protocol_impl in &self.protocol_impls {
            if !protocol_impl.is_current_program {
                continue;
            }
            if protocol_impl.type_name != receiver_root {
                continue;
            }
            if !protocol_impl
                .mappings
                .iter()
                .any(|mapping| mapping.method == method)
            {
                continue;
            }
            if let Some(sig) = self.resolve_function(Some(&protocol_impl.protocol), method) {
                let namespace = protocol_impl.protocol.clone();
                if !candidates
                    .iter()
                    .any(|(candidate_namespace, _)| candidate_namespace == &namespace)
                {
                    candidates.push((namespace, sig.clone()));
                }
            }
        }

        candidates
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

    fn insert_builtin_interface(&mut self, program: &SyntaxProgram) {
        for item in &program.items {
            match item {
                Item::Function(function) => {
                    self.insert_function(function_sig_from_decl(function, true));
                }
                Item::Type(type_decl) => {
                    self.insert_builtin_type(type_info_from_decl(type_decl));
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
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
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
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
            effect: effective_param_effect(param),
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
    // Store protocol bounds for receiver-call shorthand resolution.
    // Convention: "__protocol_bound__<TypeParam>" -> "<ProtocolName>"
    for type_param in &function.type_params {
        if let Some(GenericBound::Protocol(protocol)) = &type_param.bound {
            value_types.insert(
                format!("__protocol_bound__{}", type_param.name),
                protocol.clone(),
            );
        }
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
    let mut statements = Vec::new();
    for statement in &block.statements {
        statements.extend(lower_hir_stmts(hir, function_name, statement, value_types));
    }
    HirBlock {
        statements,
        span: block.span.clone(),
    }
}

fn lower_hir_stmts(
    hir: &Hir,
    function_name: &str,
    statement: &Stmt,
    value_types: &mut HashMap<String, String>,
) -> Vec<HirStmt> {
    match statement {
        Stmt::LetElse(stmt) => {
            let value_type_name = infer_hir_expr_type(hir, &stmt.value, value_types);
            let binding_type_name =
                match_pattern_binding_type(&stmt.pattern, value_type_name.as_deref())
                    .map(|(_, type_name)| type_name);
            let mut statements = vec![HirStmt::Match {
                value: lower_hir_expr(hir, function_name, &stmt.value, value_types),
                scrutinee_effect: None,
                arms: vec![
                    HirMatchArm {
                        pattern: stmt.pattern.clone(),
                        guard: None,
                        body: HirBlock {
                            statements: Vec::new(),
                            span: stmt.span.clone(),
                        },
                        span: stmt.span.clone(),
                    },
                    HirMatchArm {
                        pattern: MatchPattern::Wildcard(stmt.span.clone()),
                        guard: None,
                        body: {
                            let mut else_types = value_types.clone();
                            lower_hir_block(hir, function_name, &stmt.else_body, &mut else_types)
                        },
                        span: stmt.span.clone(),
                    },
                ],
                span: stmt.span.clone(),
            }];
            if !stmt.binding_name.is_empty() {
                if let Some(type_name) = &binding_type_name {
                    value_types.insert(stmt.binding_name.clone(), type_name.clone());
                }
                statements.push(HirStmt::Let {
                    kind: HirBindingKind::ManagedLet,
                    name: stmt.binding_name.clone(),
                    value: None,
                    type_name: binding_type_name,
                    value_type_name,
                    is_async: false,
                    span: stmt.span.clone(),
                });
            }
            statements
        }
        Stmt::TaskGroup(stmt) => {
            let mut body_types = value_types.clone();
            let mut statements =
                lower_hir_block(hir, function_name, &stmt.body, &mut body_types).statements;
            append_task_group_drains(&mut statements);
            statements
        }
        _ => vec![lower_hir_stmt(hir, function_name, statement, value_types)],
    }
}

/// Structured-concurrency drain for a `task_group` body. A `task_group` flattens
/// into its statements (so every checker pass sees the body transparently), but
/// the executable backends must still drain `async let` tasks that the scope
/// spawned and never awaited — leaving the group joins them so background work
/// runs to completion. The compiled backend does this via its scope guard; the
/// reg VM has no such boundary after flattening, so we make the drain explicit
/// here by appending an `await <handle>` for each un-awaited `async let`.
///
/// Only un-awaited handles are drained: the `await` checker (RS0030) consumes an
/// `async let` name the first time it is awaited, so re-awaiting an already-joined
/// handle would both be rejected and be redundant. Discard (`_`) async-lets can
/// never be awaited by name, so they are always drained — and are renamed to
/// unique handles so the appended `await` can reference them (this also fixes
/// multiple `_` handles colliding on the same register/name).
fn append_task_group_drains(statements: &mut Vec<HirStmt>) {
    let awaited = collect_awaited_handle_names(statements);
    let mut drains = Vec::new();
    let mut discard_index = 0usize;
    for statement in statements.iter_mut() {
        let HirStmt::Let {
            name,
            value: Some(_),
            is_async: true,
            span,
            ..
        } = statement
        else {
            continue;
        };
        if name == "_" {
            *name = format!("__rss_task_group_discard_{discard_index}");
            discard_index += 1;
        } else if awaited.contains(name) {
            // Already awaited in the body; the handle is consumed.
            continue;
        }
        let span = span.clone();
        drains.push(HirStmt::Expr(HirExpr::Await {
            value: Box::new(HirExpr::Ident {
                name: name.clone(),
                type_name: None,
                span: span.clone(),
            }),
            type_name: None,
            span,
        }));
    }
    statements.extend(drains);
}

/// Names of `async let` handles the body awaits at least once, so the drain can
/// skip them. `await x` and `await x?` (which wraps the ident in `Try`/`Effect`)
/// both count as awaiting `x`.
fn collect_awaited_handle_names(statements: &[HirStmt]) -> HashSet<String> {
    let mut names = HashSet::new();
    for statement in statements {
        walk_stmt_for_awaits(statement, &mut names);
    }
    names
}

fn walk_stmt_for_awaits(statement: &HirStmt, names: &mut HashSet<String>) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                walk_expr_for_awaits(value, names);
            }
        }
        HirStmt::Expr(expr) => walk_expr_for_awaits(expr, names),
        HirStmt::Assign { target, value, .. } => {
            walk_expr_for_awaits(target, names);
            walk_expr_for_awaits(value, names);
        }
        HirStmt::With { resource, body, .. } => {
            walk_expr_for_awaits(resource, names);
            walk_block_for_awaits(body, names);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            walk_expr_for_awaits(condition, names);
            walk_block_for_awaits(then_body, names);
            if let Some(else_body) = else_body {
                walk_block_for_awaits(else_body, names);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                walk_expr_for_awaits(condition, names);
            }
            walk_block_for_awaits(body, names);
        }
        HirStmt::For { iterable, body, .. } => {
            walk_expr_for_awaits(iterable, names);
            walk_block_for_awaits(body, names);
        }
        HirStmt::Match { value, arms, .. } => {
            walk_expr_for_awaits(value, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_for_awaits(guard, names);
                }
                walk_block_for_awaits(&arm.body, names);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                walk_expr_for_awaits(&arm.operation, names);
                walk_block_for_awaits(&arm.body, names);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn walk_block_for_awaits(block: &HirBlock, names: &mut HashSet<String>) {
    for statement in &block.statements {
        walk_stmt_for_awaits(statement, names);
    }
}

fn walk_expr_for_awaits(expr: &HirExpr, names: &mut HashSet<String>) {
    match expr {
        HirExpr::Await { value, .. } => {
            if let Some(name) = awaited_handle_name(value) {
                names.insert(name);
            }
            walk_expr_for_awaits(value, names);
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                walk_expr_for_awaits(&field.value, names);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                walk_expr_for_awaits(&entry.key, names);
                walk_expr_for_awaits(&entry.value, names);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                walk_expr_for_awaits(item, names);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            walk_expr_for_awaits(left, names);
            walk_expr_for_awaits(right, names);
        }
        HirExpr::Field { base, .. } => walk_expr_for_awaits(base, names),
        HirExpr::Index { base, index, .. } => {
            walk_expr_for_awaits(base, names);
            walk_expr_for_awaits(index, names);
        }
        HirExpr::Call { receiver, args, .. } => {
            if let Some(receiver) = receiver {
                walk_expr_for_awaits(&receiver.value, names);
            }
            for arg in args {
                walk_expr_for_awaits(&arg.value, names);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Try { value, .. } => walk_expr_for_awaits(value, names),
        HirExpr::Closure { body, .. } => walk_block_for_awaits(body, names),
        HirExpr::Match { value, arms, .. } => {
            walk_expr_for_awaits(value, names);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    walk_expr_for_awaits(guard, names);
                }
                walk_block_for_awaits(&arm.body, names);
            }
        }
    }
}

/// Peel `Try`/`Effect` wrappers off an awaited operand to recover the handle
/// identifier, e.g. the `x` in `await x?`.
fn awaited_handle_name(expr: &HirExpr) -> Option<String> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name.clone()),
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => awaited_handle_name(value),
        _ => None,
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
            let value_type_name = stmt
                .value
                .as_ref()
                .and_then(|value| infer_hir_expr_type(hir, value, value_types));
            let declared_type_name = stmt.type_annotation.as_ref().map(type_ref_name);
            let type_name = declared_type_name
                .clone()
                .or_else(|| value_type_name.clone());
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
                value_type_name,
                is_async: stmt.is_async,
                span: stmt.span.clone(),
            }
        }
        Stmt::Return(stmt) => {
            let proof = stmt
                .value
                .as_ref()
                .map_or(HirReturnProof::NoValue, |value| {
                    classify_return_expr(hir, value, value_types)
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
        Stmt::For(stmt) => {
            let iterable_type = infer_hir_expr_type(hir, &stmt.iterable, value_types);
            let item_type = if stmt.is_async {
                iterable_type.as_deref().and_then(stream_item_type)
            } else {
                iterable_type.as_deref().and_then(list_element_type)
            };
            let mut body_types = value_types.clone();
            if let Some(item_type) = &item_type {
                body_types.insert(stmt.binding.clone(), item_type.clone());
            }
            HirStmt::For {
                binding: stmt.binding.clone(),
                iterable: lower_hir_expr(hir, function_name, &stmt.iterable, value_types),
                iterable_type_name: iterable_type,
                item_type_name: item_type,
                is_async: stmt.is_async,
                body: lower_hir_block(hir, function_name, &stmt.body, &mut body_types),
                span: stmt.span.clone(),
            }
        }
        Stmt::Match(stmt) => {
            let value_type = infer_hir_expr_type(hir, &stmt.value, value_types);
            let value = lower_hir_expr(hir, function_name, &stmt.value, value_types);
            let arms = stmt
                .arms
                .iter()
                .map(|arm| {
                    let mut arm_types = value_types.clone();
                    for (binding, type_name) in
                        match_pattern_binding_types(hir, &arm.pattern, value_type.as_deref())
                    {
                        arm_types.insert(binding, type_name);
                    }
                    HirMatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|guard| lower_hir_expr(hir, function_name, guard, &arm_types)),
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirStmt::Match {
                value,
                scrutinee_effect: stmt.scrutinee_effect,
                arms,
                span: stmt.span.clone(),
            }
        }
        Stmt::Select(stmt) => {
            let arms = stmt
                .arms
                .iter()
                .map(|arm| {
                    // The binding observes the *awaited* value of the operation,
                    // so the body sees it with the resolved (unwrapped) type.
                    let binding_type = infer_hir_expr_type(hir, &arm.operation, value_types);
                    let operation = lower_hir_expr(hir, function_name, &arm.operation, value_types);
                    let mut arm_types = value_types.clone();
                    if arm.binding != "_"
                        && let Some(type_name) = binding_type
                    {
                        arm_types.insert(arm.binding.clone(), type_name);
                    }
                    HirSelectArm {
                        binding: arm.binding.clone(),
                        operation,
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirStmt::Select {
                arms,
                span: stmt.span.clone(),
            }
        }
        Stmt::TaskGroup(_) => unreachable!("task-group statements are lowered by lower_hir_stmts"),
        Stmt::LetElse(_) => unreachable!("let-else statements are lowered by lower_hir_stmts"),
        // Controlled assignment is checked at the AST level; in HIR it carries
        // the value expression so the RHS still gets ownership/use analysis, and
        // the lowered target so executable backends know which binding to store.
        Stmt::Assign(stmt) => HirStmt::Assign {
            target: lower_hir_expr(hir, function_name, &stmt.target, value_types),
            value: lower_hir_expr(hir, function_name, &stmt.value, value_types),
            span: stmt.span.clone(),
        },
        Stmt::Expr(expr) => HirStmt::Expr(lower_hir_expr(hir, function_name, expr, value_types)),
        Stmt::Break(span) => HirStmt::Break(span.clone()),
        Stmt::Continue(span) => HirStmt::Continue(span.clone()),
        Stmt::MalformedWith(span)
        | Stmt::MalformedIf(span)
        | Stmt::MalformedLoop(span)
        | Stmt::MalformedFor(span)
        | Stmt::MalformedMatch(span)
        | Stmt::Unknown(span) => HirStmt::Unknown(span.clone()),
    }
}

fn lower_hir_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    value_types: &HashMap<String, String>,
) -> HirExpr {
    match expr {
        // A reference to a top-level `const` is inlined to its literal value: the
        // register VM has no const/global slots, and the literal carries the value
        // to every backend. A local binding of the same name shadows the const.
        Expr::Ident(name, _)
            if !value_types.contains_key(name) && hir.const_values.contains_key(name) =>
        {
            let value = hir.const_values[name].clone();
            lower_hir_expr(hir, function_name, &value, value_types)
        }
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
        Expr::MultilineString(value, span) => HirExpr::String {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::CharLiteral(value, span) => HirExpr::Char {
            value: value.clone(),
            span: span.clone(),
        },
        Expr::ObjectLiteral { fields, span } => HirExpr::ObjectLiteral {
            fields: fields
                .iter()
                .map(|field| HirObjectLiteralField {
                    name: field.name.clone(),
                    value: lower_hir_expr(hir, function_name, &field.value, value_types),
                    span: field.span.clone(),
                })
                .collect(),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::MapLiteral { entries, span } => HirExpr::MapLiteral {
            entries: entries
                .iter()
                .map(|entry| HirMapLiteralEntry {
                    key: lower_hir_expr(hir, function_name, &entry.key, value_types),
                    value: lower_hir_expr(hir, function_name, &entry.value, value_types),
                    span: entry.span.clone(),
                })
                .collect(),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::ArrayLiteral { items, span } => HirExpr::ArrayLiteral {
            items: items
                .iter()
                .map(|item| lower_hir_expr(hir, function_name, item, value_types))
                .collect(),
            type_name: infer_hir_expr_type(hir, expr, value_types),
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
            let resolved = base_type.as_deref().and_then(|type_name| {
                let type_info = hir.type_info(type_name)?;
                let field = type_info.fields.get(name)?;
                Some((type_info, type_name, field))
            });
            HirExpr::Field {
                base: Box::new(lower_hir_expr(hir, function_name, base, value_types)),
                name: name.clone(),
                access: HirFieldAccess {
                    function_name: function_name.to_string(),
                    name: name.clone(),
                    span: span.clone(),
                    type_name: resolved.map(|(type_info, type_name, field)| {
                        substituted_field_type(type_info, type_name, field)
                    }),
                    is_handle: resolved
                        .is_some_and(|(_, _, field)| field.is_handle || field.is_weak),
                    is_weak: resolved.is_some_and(|(_, _, field)| field.is_weak),
                    base_type,
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
            lower_hir_call_expr(hir, function_name, expr, callee, args, span, value_types)
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
        Expr::Spawn { value, span } => HirExpr::Spawn {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::Await { value, span } => HirExpr::Await {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::Try { value, span } => HirExpr::Try {
            value: Box::new(lower_hir_expr(hir, function_name, value, value_types)),
            type_name: infer_hir_expr_type(hir, expr, value_types),
            span: span.clone(),
        },
        Expr::Closure {
            params,
            captures,
            declared_effects,
            explicit,
            body,
            span,
        } => {
            let mut closure_types = value_types.clone();
            HirExpr::Closure {
                params: params.clone(),
                captures: captures
                    .iter()
                    .map(|capture| HirClosureCapture {
                        effect: param_effect_from_data_effect(capture.effect),
                        name: capture.name.clone(),
                        span: capture.span.clone(),
                    })
                    .collect(),
                declared_effects: declared_effects.clone(),
                explicit: *explicit,
                body: lower_hir_block(hir, function_name, body, &mut closure_types),
                span: span.clone(),
            }
        }
        Expr::Match {
            value,
            scrutinee_effect,
            arms,
            span,
            ..
        } => {
            let value_type = infer_hir_expr_type(hir, value, value_types);
            let lowered_value = lower_hir_expr(hir, function_name, value, value_types);
            let mut match_type = None;
            let lowered_arms = arms
                .iter()
                .map(|arm| {
                    let mut arm_types = value_types.clone();
                    for (binding, type_name) in
                        match_pattern_binding_types(hir, &arm.pattern, value_type.as_deref())
                    {
                        arm_types.insert(binding, type_name);
                    }
                    if match_type.is_none() {
                        match_type = infer_closure_return_type(hir, &arm.body, &arm_types);
                    }
                    HirMatchArm {
                        pattern: arm.pattern.clone(),
                        guard: arm
                            .guard
                            .as_ref()
                            .map(|guard| lower_hir_expr(hir, function_name, guard, &arm_types)),
                        body: lower_hir_block(hir, function_name, &arm.body, &mut arm_types),
                        span: arm.span.clone(),
                    }
                })
                .collect();
            HirExpr::Match {
                value: Box::new(lowered_value),
                scrutinee_effect: *scrutinee_effect,
                arms: lowered_arms,
                type_name: match_type,
                span: span.clone(),
            }
        }
        Expr::Unknown(span) => HirExpr::Unknown(span.clone()),
    }
}

/// Lowers an `Expr::Call`: receiver type inference, call resolution, retain
/// events, and default-parameter synthesis. Extracted from `lower_hir_expr`;
/// `expr` is the original `Expr::Call` node (needed for type inference).
fn lower_hir_call_expr(
    hir: &Hir,
    function_name: &str,
    expr: &Expr,
    callee: &Callee,
    args: &[CallArg],
    span: &Span,
    value_types: &HashMap<String, String>,
) -> HirExpr {
    let receiver_type = match callee {
        Callee::ReceiverCall { receiver, .. } => infer_hir_expr_type(hir, receiver, value_types),
        _ => None,
    };
    let (resolution, resolved_namespace) = match callee {
        Callee::ReceiverCall { method, .. } => {
            if let Some(receiver_type) = receiver_type.as_deref() {
                hir.resolve_receiver_call(receiver_type, method, value_types)
            } else {
                (CallResolution::Unknown, None)
            }
        }
        _ => (hir.resolve_call(callee), None),
    };
    let events = retain_events_for_call(
        function_name,
        callee,
        args,
        span,
        &resolution,
        hir,
        value_types,
    );
    let type_name = infer_hir_expr_type(hir, expr, value_types);
    let mut hir_args: Vec<HirCallArg> = args
        .iter()
        .map(|arg| HirCallArg {
            name: arg.name.clone(),
            value: lower_hir_expr(hir, function_name, &arg.value, value_types),
            span: arg.span.clone(),
        })
        .collect();
    // Syntax preserves whether a caller wrote `read`, but the semantic form has
    // exactly one representation: a bare argument for a read parameter is the
    // same read effect as an explicitly wrapped argument. This happens after
    // resolution so a bare argument can never be upgraded to mut/take.
    if let CallResolution::Resolved { signature, .. } = &resolution {
        let positional_offset = usize::from(matches!(callee, Callee::ReceiverCall { .. }));
        for (index, arg) in hir_args.iter_mut().enumerate() {
            let actual_type = args
                .get(index)
                .and_then(|source_arg| infer_hir_expr_type(hir, &source_arg.value, value_types));
            let positional_param = signature.params.get(index + positional_offset);
            // Same-name punning is deliberately narrower than positional calls:
            // only an identifier identical to a resolved default-read parameter
            // may omit its label. Other unnamed arguments remain diagnostics.
            if arg.name.is_none()
                && let (Some(param), HirExpr::Ident { name, .. }) = (positional_param, &arg.value)
                && param.name == *name
                && param.effect == Some(ParamEffect::Read)
            {
                arg.name = Some(name.clone());
            }
            let expected = arg
                .name
                .as_deref()
                .and_then(|name| signature.params.iter().find(|param| param.name == name))
                .or(positional_param);
            let Some(param) = expected else {
                continue;
            };
            if param.effect == Some(ParamEffect::Read) && !hir_expr_already_read(&arg.value) {
                let value = std::mem::replace(&mut arg.value, HirExpr::Unknown(arg.span.clone()));
                arg.value = HirExpr::Effect {
                    effect: ParamEffect::Read,
                    value: Box::new(value),
                    events: Vec::new(),
                    type_name: actual_type.or_else(|| Some(param.type_name.clone())),
                    span: arg.span.clone(),
                };
            }
        }
    }
    // Fill omitted parameters that declare a default value, so every
    // backend sees a complete call (defaults are desugared once, here).
    if let CallResolution::Resolved { signature, .. } = &resolution {
        let provided: std::collections::HashSet<String> =
            hir_args.iter().filter_map(|arg| arg.name.clone()).collect();
        for param in &signature.params {
            if let Some(default) = &param.default
                && !provided.contains(&param.name)
            {
                let mut value = lower_hir_expr(hir, function_name, default, value_types);
                // A non-Copy default is materialized at the call site and
                // bound under the parameter's declared effect. Carry that
                // effect on the synthesized argument so the call-site
                // effect check is satisfied — the effect is reviewed at the
                // declaration (`axes: read List<Int> = ...`), not at the
                // omitted call where the argument is implicit.
                if let Some(effect) = param.effect {
                    value = HirExpr::Effect {
                        effect,
                        value: Box::new(value),
                        events: Vec::new(),
                        type_name: Some(param.type_name.clone()),
                        span: span.clone(),
                    };
                }
                hir_args.push(HirCallArg {
                    name: Some(param.name.clone()),
                    value,
                    span: span.clone(),
                });
            }
        }
    }
    HirExpr::Call {
        callee: callee.clone(),
        receiver: match callee {
            Callee::ReceiverCall {
                receiver, effect, ..
            } => Some(HirCallReceiver {
                value: Box::new(lower_hir_expr(hir, function_name, receiver, value_types)),
                effect: param_effect_from_data_effect((*effect).unwrap_or(DataEffect::Read)),
                type_name: receiver_type,
                resolved_namespace,
            }),
            _ => None,
        },
        args: hir_args,
        type_name,
        resolution,
        events,
        span: span.clone(),
    }
}

fn effect_events_for_expr(function_name: &str, expr: &Expr) -> Vec<HirEffectEvent> {
    let event = match expr {
        Expr::Manage { value, span } => {
            let Some((binding_name, value_span)) = direct_move_binding(value) else {
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
            let Some((binding_name, value_span)) = direct_move_binding(value) else {
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
    hir: &Hir,
    value_types: &HashMap<String, String>,
) -> Vec<HirEffectEvent> {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return Vec::new();
    };
    if signature.retained_params.is_empty() {
        return Vec::new();
    }

    let positional_offset = usize::from(matches!(callee, Callee::ReceiverCall { .. }));
    args.iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            let param = arg
                .name
                .as_deref()
                .and_then(|name| signature.params.iter().find(|param| param.name == name))
                .or_else(|| signature.params.get(index + positional_offset))?;
            let name = &param.name;
            if !signature.retained_params.contains(name) {
                return None;
            }
            let retained =
                direct_effect_retained_binding(&arg.value, hir, value_types).or_else(|| {
                    (param.effect == Some(ParamEffect::Read))
                        .then(|| retained_inline_binding(&arg.value, hir, value_types))
                        .flatten()
                });
            let (binding_name, value_span) = retained?;
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
            if stmt.is_async {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function_name.to_string()),
                    kind: HirFeatureUseKind::Async,
                    span: stmt.span.clone(),
                });
            }
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
            let value_type_name = stmt
                .value
                .as_ref()
                .and_then(|value| infer_hir_expr_type(hir, value, value_types));
            let declared_type_name = stmt.type_annotation.as_ref().map(type_ref_name);
            let type_name = declared_type_name
                .clone()
                .or_else(|| value_type_name.clone());
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
                    proof: classify_return_expr(hir, value, value_types),
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
        Stmt::For(stmt) => {
            if stmt.is_async {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function_name.to_string()),
                    kind: HirFeatureUseKind::Async,
                    span: stmt.span.clone(),
                });
            }
            collect_body_facts_in_expr(hir, function_name, &stmt.iterable, value_types, facts);
            let iterable_type = infer_hir_expr_type(hir, &stmt.iterable, value_types);
            let item_type = if stmt.is_async {
                iterable_type.as_deref().and_then(stream_item_type)
            } else {
                iterable_type.as_deref().and_then(list_element_type)
            };
            let mut body_types = value_types.clone();
            if let Some(item_type) = item_type {
                facts.bindings.push(HirBinding {
                    function_name: function_name.to_string(),
                    name: stmt.binding.clone(),
                    kind: HirBindingKind::ManagedLet,
                    effect: None,
                    span: stmt.span.clone(),
                    type_name: Some(item_type.clone()),
                });
                body_types.insert(stmt.binding.clone(), item_type);
            }
            collect_body_facts_in_block(hir, function_name, &stmt.body, &mut body_types, facts);
        }
        Stmt::TaskGroup(stmt) => {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function_name.to_string()),
                kind: HirFeatureUseKind::Async,
                span: stmt.span.clone(),
            });
            let mut body_types = value_types.clone();
            collect_body_facts_in_block(hir, function_name, &stmt.body, &mut body_types, facts);
        }
        Stmt::Select(stmt) => {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function_name.to_string()),
                kind: HirFeatureUseKind::Async,
                span: stmt.span.clone(),
            });
            for arm in &stmt.arms {
                collect_body_facts_in_expr(hir, function_name, &arm.operation, value_types, facts);
                let binding_type = infer_hir_expr_type(hir, &arm.operation, value_types);
                let mut arm_types = value_types.clone();
                if arm.binding != "_"
                    && let Some(type_name) = binding_type
                {
                    facts.bindings.push(HirBinding {
                        function_name: function_name.to_string(),
                        name: arm.binding.clone(),
                        kind: HirBindingKind::ManagedLet,
                        effect: None,
                        span: arm.span.clone(),
                        type_name: Some(type_name.clone()),
                    });
                    arm_types.insert(arm.binding.clone(), type_name);
                }
                collect_body_facts_in_block(hir, function_name, &arm.body, &mut arm_types, facts);
            }
        }
        Stmt::Match(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
            let value_type = infer_hir_expr_type(hir, &stmt.value, value_types);
            for arm in &stmt.arms {
                let mut arm_types = value_types.clone();
                for (binding, type_name) in
                    match_pattern_binding_types(hir, &arm.pattern, value_type.as_deref())
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
        Stmt::LetElse(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
            let mut else_types = value_types.clone();
            collect_body_facts_in_block(
                hir,
                function_name,
                &stmt.else_body,
                &mut else_types,
                facts,
            );
            if let Some((binding, type_name)) = match_pattern_binding_type(
                &stmt.pattern,
                infer_hir_expr_type(hir, &stmt.value, value_types).as_deref(),
            ) {
                facts.bindings.push(HirBinding {
                    function_name: function_name.to_string(),
                    name: binding.clone(),
                    kind: HirBindingKind::ManagedLet,
                    effect: None,
                    span: stmt.span.clone(),
                    type_name: Some(type_name.clone()),
                });
                value_types.insert(binding, type_name);
            }
        }
        Stmt::Assign(stmt) => {
            collect_body_facts_in_expr(hir, function_name, &stmt.target, value_types, facts);
            collect_body_facts_in_expr(hir, function_name, &stmt.value, value_types, facts);
        }
        Stmt::Expr(expr) => {
            collect_body_facts_in_expr(hir, function_name, expr, value_types, facts);
        }
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
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
            let resolution = match callee {
                Callee::ReceiverCall {
                    receiver, method, ..
                } => {
                    if let Some(receiver_type) = infer_hir_expr_type(hir, receiver, value_types) {
                        let (res, _namespace) =
                            hir.resolve_receiver_call(&receiver_type, method, value_types);
                        res
                    } else {
                        CallResolution::Unknown
                    }
                }
                _ => hir.resolve_call(callee),
            };
            if matches!(
                &resolution,
                CallResolution::Resolved { signature, .. } if signature.is_async
            ) {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function_name.to_string()),
                    kind: HirFeatureUseKind::Async,
                    span: span.clone(),
                });
            }
            if matches!(
                &resolution,
                CallResolution::Resolved { signature, .. }
                    if signature.effects.iter().any(|effect| effect == "unsafe")
            ) {
                facts.feature_uses.push(HirFeatureUse {
                    function_name: Some(function_name.to_string()),
                    kind: HirFeatureUseKind::Unsafe,
                    span: span.clone(),
                });
            }
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
            facts.effect_events.extend(retain_events_for_call(
                function_name,
                callee,
                args,
                span,
                &resolution,
                hir,
                value_types,
            ));
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
            if let Some((binding_name, value_span)) = direct_move_binding(value) {
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
        Expr::Spawn { value, span } => {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function_name.to_string()),
                kind: HirFeatureUseKind::Async,
                span: span.clone(),
            });
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
        }
        Expr::Await { value, span } => {
            facts.feature_uses.push(HirFeatureUse {
                function_name: Some(function_name.to_string()),
                kind: HirFeatureUseKind::Async,
                span: span.clone(),
            });
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
            let resolved = base_type.as_deref().and_then(|type_name| {
                let type_info = hir.type_info(type_name)?;
                let field = type_info.fields.get(name)?;
                Some((type_info, type_name, field))
            });
            facts.field_accesses.push(HirFieldAccess {
                function_name: function_name.to_string(),
                name: name.clone(),
                span: span.clone(),
                type_name: resolved.map(|(type_info, type_name, field)| {
                    substituted_field_type(type_info, type_name, field)
                }),
                is_handle: resolved.is_some_and(|(_, _, field)| field.is_handle || field.is_weak),
                is_weak: resolved.is_some_and(|(_, _, field)| field.is_weak),
                base_type,
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
        Expr::Match { value, arms, .. } => {
            collect_body_facts_in_expr(hir, function_name, value, value_types, facts);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_body_facts_in_expr(hir, function_name, guard, value_types, facts);
                }
                collect_body_facts_in_block(hir, function_name, &arm.body, value_types, facts);
            }
        }
        Expr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_body_facts_in_expr(hir, function_name, &field.value, value_types, facts);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_body_facts_in_expr(hir, function_name, &entry.key, value_types, facts);
                collect_body_facts_in_expr(hir, function_name, &entry.value, value_types, facts);
            }
        }
        Expr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_body_facts_in_expr(hir, function_name, item, value_types, facts);
            }
        }
        Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn is_resource_pool_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Name(name) if type_root_name(name) == "ResourcePool")
        || matches!(callee, Callee::Qualified { namespace, .. } if type_root_name(namespace) == "ResourcePool")
}

fn direct_effect_retained_binding(
    expr: &Expr,
    hir: &Hir,
    value_types: &HashMap<String, String>,
) -> Option<(String, Span)> {
    match expr {
        Expr::Effect { value, .. } => retained_inline_binding(value, hir, value_types),
        _ => None,
    }
}

fn retained_inline_binding(
    expr: &Expr,
    hir: &Hir,
    value_types: &HashMap<String, String>,
) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            retained_inline_binding(value, hir, value_types)
        }
        Expr::Field { base, name, span } => {
            let base_type = infer_hir_expr_type(hir, base, value_types)?;
            let field = hir.type_info(&base_type)?.fields.get(name)?;
            if field.is_handle || field.is_weak {
                return None;
            }
            let (binding_name, _) = retained_inline_binding(base, hir, value_types)?;
            Some((binding_name, span.clone()))
        }
        Expr::Call { callee, args, .. } if retained_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| retained_inline_binding(&arg.value, hir, value_types)),
        _ => None,
    }
}

fn retained_wrapper_callee(callee: &Callee) -> bool {
    matches!(callee_name(callee), "Ok" | "Err" | "Some")
}

fn direct_ident(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        _ => None,
    }
}

fn direct_move_binding(expr: &Expr) -> Option<(String, Span)> {
    match expr {
        Expr::Ident(name, span) => Some((name.clone(), span.clone())),
        Expr::Field { base, name, span } => {
            let (mut base_path, _) = direct_move_binding(base)?;
            base_path.push('.');
            base_path.push_str(name);
            Some((base_path, span.clone()))
        }
        _ => None,
    }
}

fn hir_binding_kind(kind: LetKind) -> HirBindingKind {
    match kind {
        LetKind::Managed => HirBindingKind::ManagedLet,
        LetKind::Local => HirBindingKind::LocalLet,
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

pub(super) fn callee_name(callee: &Callee) -> &str {
    match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => type_root_name(name),
        Callee::ReceiverCall { method, .. } => method.as_str(),
    }
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            (*effect).map(|e| e.as_str()).unwrap_or("read"),
            receiver_call_label(receiver)
        ),
    }
}

fn receiver_call_label(receiver: &Expr) -> String {
    match receiver {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Field { base, name, .. } => format!("{}.{}", receiver_call_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", receiver_call_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            receiver_call_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

fn receiver_facade_namespace(receiver_root: &str, method: &str) -> Option<&'static str> {
    match receiver_root {
        "JsonValue" | "JsonLiteral" => Some("Json"),
        "String" if method.starts_with("json_") => Some("Json"),
        _ => None,
    }
}

fn function_sig_from_decl(function: &FunctionDecl, is_builtin: bool) -> FunctionSig {
    let (namespace, name) = split_function_name(&function.name);
    FunctionSig {
        namespace,
        name,
        is_public: function.is_public,
        is_async: function.is_async,
        is_native: function.is_native,
        type_params: function
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        type_param_bounds: function
            .type_params
            .iter()
            .map(|param| param.bound.clone())
            .collect(),
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
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = match ty.effective_fn_param_effect(index) {
                    Some(effect) => format!("{} ", effect.as_str()),
                    None => String::new(),
                };
                format!("{prefix}{}", type_ref_name(param))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
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
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
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
        effect: effective_param_effect(param),
        type_name: type_ref_name(&param.ty),
        default: param.default.clone(),
    }
}

/// Syntax records whether the author wrote an effect. Semantics add the silent
/// `read` default for ordinary data values; the established closure/Fd
/// exceptions continue to pass without a data-effect capability.
fn effective_param_effect(param: &Param) -> Option<ParamEffect> {
    param.effective_effect().map(param_effect_from_data_effect)
}

fn hir_expr_already_read(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Effect { .. } => true,
        HirExpr::Call {
            callee: Callee::ReceiverCall { effect, .. },
            ..
        } => matches!(effect, Some(DataEffect::Read) | None),
        _ => false,
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
    let fields_ordered = type_decl
        .fields
        .iter()
        .map(field_info_from_decl)
        .collect::<Vec<_>>();
    TypeInfo {
        name: type_decl.name.clone(),
        kind: type_kind_from_decl(type_decl.kind),
        type_params: type_decl
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        fields: fields_ordered
            .iter()
            .map(|field| (field.name.clone(), field.clone()))
            .collect(),
        fields_ordered,
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
        default: field.default.clone(),
    }
}

fn constructor_sig_from_type(type_info: &TypeInfo, is_builtin: bool) -> FunctionSig {
    FunctionSig {
        namespace: None,
        name: type_info.name.clone(),
        is_public: is_builtin,
        is_async: false,
        is_native: false,
        type_params: type_info.type_params.clone(),
        type_param_bounds: vec![None; type_info.type_params.len()],
        params: type_info
            .fields_ordered
            .iter()
            .map(|field| ParamSig {
                name: field.name.clone(),
                effect: None,
                type_name: field.type_name.clone(),
                default: field.default.clone(),
            })
            .collect(),
        // A generic struct's constructor returns the type *applied to its params*
        // (`Wrap<T>`), not the bare name (`Wrap`). Carrying the params lets
        // `infer_signature_return_type` substitute them from the arguments
        // (`Wrap(item: 7)` -> `Wrap<Int>`); a bare name leaves nothing to
        // substitute and spuriously rejects `let w: Wrap<Int> = Wrap(item: 7)`.
        return_type: Some(if type_info.type_params.is_empty() {
            type_info.name.clone()
        } else {
            format!("{}<{}>", type_info.name, type_info.type_params.join(", "))
        }),
        returns_fresh: type_info.kind == HirTypeKind::Struct,
        effects: Vec::new(),
        retained_params: HashSet::new(),
        is_builtin,
    }
}

fn qualified_key(namespace: &str, name: &str) -> String {
    format!("{namespace}.{name}")
}

/// The evaluated sub-expressions of an assignment *target* (the place on the left
/// of `=`), so a checker pass can analyze them like any other expression. The
/// write root itself is excluded (assigning to `x` *defines* `x`, it doesn't read
/// it), but a field/index base *is* read to reach the place, and an index
/// expression is arbitrary evaluated code. So:
///   `x = v`        -> [] (pure write)
///   `x.field = v`  -> [base]                 (base is read)
///   `xs[i] = v`    -> [base, index]          (base read, index evaluated)
/// Nested places recurse naturally because the base is itself a `Field`/`Index`.
/// Used by passes that previously only inspected the assigned `value`, missing
/// awaits, `?`, moves, etc. inside the target (e.g. `xs[await f()] = v`).
pub(crate) fn assign_target_reads(target: &HirExpr) -> Vec<&HirExpr> {
    match target {
        HirExpr::Ident { .. } => Vec::new(),
        HirExpr::Field { base, .. } => vec![base.as_ref()],
        HirExpr::Index { base, index, .. } => vec![base.as_ref(), index.as_ref()],
        // Defensive: any other target shape is checked as a whole expression.
        other => vec![other],
    }
}
