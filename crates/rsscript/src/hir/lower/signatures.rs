//! HIR construction, signatures, aliases, and semantic type table population.

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
        let mut hir = Self {
            semantic_types: Arc::new(SemanticTypeFacts::from_programs(
                program,
                builtin_interfaces.iter().chain(interfaces),
            )),
            ..Self::default()
        };
        for interface in builtin_interfaces {
            hir.insert_builtin_interface(interface);
        }
        let mut type_symbols: HashMap<String, (DuplicateSymbolKind, Span)> = HashMap::new();
        let mut callable_symbols: HashMap<String, (DuplicateSymbolKind, Span)> = HashMap::new();
        for interface in interfaces {
            hir.extend_protocol_impls(&interface.protocol_impls, false);
            hir.collect_item_signatures(interface, &mut type_symbols, &mut callable_symbols, true);
        }
        hir.extend_protocol_impls(&program.protocol_impls, true);
        hir.collect_item_signatures(program, &mut type_symbols, &mut callable_symbols, false);
        hir.normalize_class_typed_handle_fields();
        hir.collect_const_values(program);
        hir.collect_resource_drop_bodies(program);
        hir.collect_body_facts(program);
        hir
    }

    pub(crate) fn semantic_types_arc(&self) -> Arc<SemanticTypeFacts> {
        Arc::clone(&self.semantic_types)
    }

    /// Record top-level `const` initializers so references can be inlined during
    /// lowering (the register VM has no const/global slots). Initializers are
    /// literals (the checker enforces this), so inlining is exact.
    pub(in crate::hir) fn collect_const_values(&mut self, program: &SyntaxProgram) {
        for item in &program.items {
            if let Item::Const(decl) = item {
                self.const_values
                    .insert(decl.name.clone(), decl.value.clone());
            }
        }
    }

    pub(in crate::hir) fn from_syntax_with_interfaces_options(
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

    pub(in crate::hir) fn extend_protocol_impls(
        &mut self,
        impls: &[ProtocolImpl],
        is_current_program: bool,
    ) {
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
    pub(in crate::hir) fn normalize_class_typed_handle_fields(&mut self) {
        let class_types: HashSet<String> = self
            .types
            .iter()
            .filter(|(_, info)| info.kind == HirTypeKind::Class)
            .map(|(name, _)| name.clone())
            .collect();
        let aliased_class_fields = self
            .types
            .values()
            .flat_map(|info| info.fields.values())
            .filter(|field| {
                class_types.contains(type_root_name(
                    &self.expand_type_alias(&field.ty.to_string()),
                ))
            })
            .map(|field| field.ty.clone())
            .collect::<HashSet<_>>();
        for info in self.types.values_mut() {
            for field in info.fields.values_mut() {
                if !field.is_handle && !field.is_weak && aliased_class_fields.contains(&field.ty) {
                    field.is_handle = true;
                }
            }
            for field in &mut info.fields_ordered {
                if !field.is_handle && !field.is_weak && aliased_class_fields.contains(&field.ty) {
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

    pub(in crate::hir) fn collect_item_signatures(
        &mut self,
        program: &SyntaxProgram,
        type_symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
        callable_symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
        is_external: bool,
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
                    self.insert_function(function_sig_from_decl(function, false, is_external));
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
                Item::TypeAlias(alias) => {
                    self.type_aliases.insert(
                        alias.name.clone(),
                        (
                            alias
                                .type_params
                                .iter()
                                .map(|parameter| parameter.name.clone())
                                .collect(),
                            type_ref_name(&alias.target),
                        ),
                    );
                }
                Item::Const(_) | Item::Module(_) | Item::Use(_) => {}
            }
        }
    }

    pub(in crate::hir) fn collect_resource_drop_bodies(&mut self, program: &SyntaxProgram) {
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
                .map(|field| (field.name.clone(), ResolvedType::from_type_ref(&field.ty)))
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
    /// call by the receiver's runtime type (dynamic protocol values + generic bounds),
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
    pub(in crate::hir) fn fields_named(
        &self,
        field_name: &str,
    ) -> impl Iterator<Item = &FieldInfo> {
        self.fields_by_name
            .get(field_name)
            .into_iter()
            .flat_map(|fields| fields.iter())
    }

    #[cfg(test)]
    pub(in crate::hir) fn is_handle_field_name(&self, field_name: &str) -> bool {
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
            signature: Box::new(signature.clone()),
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
        let receiver_type = self.expand_type_alias(receiver_type);
        let candidates = self.receiver_call_candidates(&receiver_type, method, value_types);
        if candidates.is_empty() {
            // A declared (user) type only gets the synthesized `.clone()` if it derives `Clone`;
            // otherwise leave the call unresolved (RS0206) instead of emitting an `.clone()` that
            // Rust would reject (E0599). Non-user receivers keep their existing resolution.
            let clone_allowed = {
                let root = type_root_name(&receiver_type);
                !self.types.contains_key(root) || self.clone_types.contains(root)
            };
            if method == "clone" && clone_allowed {
                // Every value supports an explicit `.clone()` returning a fresh copy of the
                // receiver's type. The runtime already clones implicitly (e.g. `read` args stored
                // into a collection); this exposes that as a callable for any `derives(Clone)` type.
                return (
                    CallResolution::Resolved {
                        signature: Box::new(FunctionSig {
                            namespace: Some(type_root_name(&receiver_type).to_string()),
                            name: "clone".to_string(),
                            is_public: true,
                            is_async: false,
                            type_params: Box::from([]),
                            type_param_bounds: Vec::new(),
                            params: vec![ParamSig {
                                name: "self".to_string(),
                                effect: Some(ParamEffect::Read),
                                ty: ResolvedType::from_display(&receiver_type),
                                default: None,
                            }],
                            return_ty: Some(ResolvedType::from_display(&receiver_type)),
                            returns_fresh: true,
                            retained_params: HashSet::new(),
                            is_builtin: true,
                            is_external: false,
                        }),
                        kind: ResolvedCalleeKind::BuiltinFunction,
                    },
                    Some(type_root_name(&receiver_type).to_string()),
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
                signature: Box::new(sig.clone()),
                kind: function_kind(sig),
            },
            Some(namespace.clone()),
        )
    }

    pub(crate) fn canonical_type_name(&self, type_name: &str) -> String {
        self.expand_type_alias(type_name)
    }

    pub(in crate::hir) fn expand_type_alias(&self, type_name: &str) -> String {
        self.expand_type_alias_inner(type_name, &mut std::collections::BTreeSet::new())
    }

    pub(in crate::hir) fn expand_type_alias_inner(
        &self,
        type_name: &str,
        visiting: &mut std::collections::BTreeSet<String>,
    ) -> String {
        let trimmed = type_name.trim();
        let prefixed = ["fresh ", "noescape ", "owned "]
            .into_iter()
            .find_map(|prefix| trimmed.strip_prefix(prefix).map(|target| (prefix, target)));
        if let Some((prefix, target)) = prefixed {
            format!("{prefix}{}", self.expand_type_alias_inner(target, visiting))
        } else {
            let root = type_root_name(trimmed);
            let expanded_args = type_arg_names(trimmed).map(|args| {
                args.into_iter()
                    .map(|argument| self.expand_type_alias_inner(argument, visiting))
                    .collect::<Vec<_>>()
            });
            let normalized = expanded_args.as_ref().map_or_else(
                || trimmed.to_string(),
                |args| format!("{root}<{}>", args.join(", ")),
            );
            if let Some((params, target)) = self.type_aliases.get(root) {
                let alias_target = if params.is_empty() {
                    Some(target.clone())
                } else {
                    expanded_args.as_ref().and_then(|args| {
                        if args.len() != params.len() {
                            return None;
                        }
                        let substitutions = params
                            .iter()
                            .cloned()
                            .zip(args.iter().cloned())
                            .collect::<HashMap<_, _>>();
                        Some(crate::text_util::substitute_type_args(
                            target,
                            &substitutions,
                        ))
                    })
                };
                if let Some(alias_target) = alias_target {
                    if !visiting.insert(root.to_string()) {
                        return normalized;
                    }
                    let expanded = self.expand_type_alias_inner(&alias_target, visiting);
                    visiting.remove(root);
                    return expanded;
                }
            }
            normalized
        }
    }

    pub(in crate::hir) fn receiver_call_candidates(
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
        if let Some(protocol) = dyn_protocol(&ResolvedType::from_display(receiver_type))
            && let Some(sig) = self.resolve_function(Some(&protocol), method)
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

    /// Structural HIR path for receiver resolution. Display strings are only
    /// projected at the legacy resolver boundary used by review/backends.
    pub(crate) fn resolve_receiver_call_structured(
        &self,
        receiver_type: &ResolvedType,
        method: &str,
        value_types: &HirValueTypes,
    ) -> (CallResolution, Option<String>) {
        let display_types = value_types
            .iter()
            .map(|(name, ty)| (name.clone(), ty.to_string()))
            .collect();
        self.resolve_receiver_call(&receiver_type.to_string(), method, &display_types)
    }

    pub(in crate::hir) fn insert_function(&mut self, signature: FunctionSig) {
        let key = match &signature.namespace {
            Some(namespace) => qualified_key(namespace, &signature.name),
            None => signature.name.clone(),
        };
        self.signatures.insert(key, signature);
    }

    pub(in crate::hir) fn insert_type(&mut self, type_info: TypeInfo) {
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

    pub(in crate::hir) fn insert_builtin_interface(&mut self, program: &SyntaxProgram) {
        for item in &program.items {
            match item {
                Item::Function(function) => {
                    self.insert_function(function_sig_from_decl(function, true, false));
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

    pub(in crate::hir) fn insert_builtin_type(&mut self, type_info: TypeInfo) {
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

    pub(in crate::hir) fn collect_body_facts(&mut self, program: &SyntaxProgram) {
        let mut facts = BodyFacts::default();
        for item in &program.items {
            match item {
                Item::Function(function) => collect_function_body_facts(self, function, &mut facts),
                Item::Type(_) => {}
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
    }
}
