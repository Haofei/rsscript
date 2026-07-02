use super::*;

pub fn review_map_sources(sources: Vec<(&str, &str)>) -> ReviewMap {
    let interfaces = standard_package_interfaces().collect::<Vec<_>>();
    review_map_sources_with_interfaces(sources, &interfaces)
}

pub(crate) fn review_map_sources_with_interfaces(
    sources: Vec<(&str, &str)>,
    interfaces: &[(&str, &str)],
) -> ReviewMap {
    let parsed_sources = sources
        .into_iter()
        .map(|(file, source)| ReviewMapParsedSource {
            file: file.to_string(),
            total_lines: source.lines().count().max(1),
            program: parse_source(file, source),
        })
        .collect::<Vec<_>>();
    let merged_program = merge_programs(parsed_sources.iter().map(|source| source.program.clone()));
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&merged_program, &interface_programs);
    let mut region_drafts = parsed_sources
        .iter()
        .flat_map(|source| review_map_file_region_drafts(source, &hir))
        .collect::<Vec<_>>();
    propagate_review_map_call_classifications(&mut region_drafts);

    let files = parsed_sources
        .iter()
        .map(|source| {
            let regions = region_drafts
                .iter()
                .filter(|draft| draft.file == source.file)
                .map(|draft| draft.region.clone())
                .collect();
            let features = feature_names(&source.program.features);
            ReviewMapFile {
                file: source.file.clone(),
                features,
                risk: review_map_file_risk(&source.program.features),
                reasons: review_map_file_reasons(&source.program.features),
                regions,
            }
        })
        .collect::<Vec<_>>();

    ReviewMap {
        summary: review_map_summary(&files),
        modules: review_map_modules(&parsed_sources),
        files,
    }
}

pub fn format_review_map_human(map: &ReviewMap) -> String {
    if map
        .files
        .iter()
        .all(|file| file.regions.is_empty() && file.reasons.is_empty())
    {
        return "review map: no functions detected\n".to_string();
    }

    let mut output = String::new();
    output.push_str(&format!(
        "summary: must-review {} functions/{} lines; low-semantic-risk {} functions/{} lines; unknown {} functions/{} lines; total {} functions/{} lines\n",
        map.summary.review_required.functions,
        map.summary.review_required.lines,
        map.summary.foldable.functions,
        map.summary.foldable.lines,
        map.summary.unknown.functions,
        map.summary.unknown.lines,
        map.summary.total_functions,
        map.summary.total_lines
    ));
    for file in &map.files {
        output.push_str(&format!("{}:", file.file));
        if !file.features.is_empty() {
            output.push_str(&format!(
                " features {}; risk {}",
                file.features.join(", "),
                review_map_file_risk_label(file.risk)
            ));
        }
        if !file.reasons.is_empty() {
            output.push_str(&format!("; {}", file.reasons.join("; ")));
        }
        output.push('\n');
        for region in &file.regions {
            output.push_str(&format!(
                "  {} [{}] line {} ({} lines): {}\n",
                region.function,
                review_map_classification_label(region.classification),
                region.line,
                region.line_count,
                region.reasons.join("; ")
            ));
        }
    }
    output
}

pub fn format_review_map_json(map: &ReviewMap) -> String {
    serde_json::to_string(map).expect("review map JSON serialization should not fail")
}

pub(super) fn review_map_summary(files: &[ReviewMapFile]) -> ReviewMapSummary {
    let mut summary = ReviewMapSummary::default();
    for region in files.iter().flat_map(|file| file.regions.iter()) {
        summary.total_functions += 1;
        summary.total_lines += region.line_count;
        let category = match region.classification {
            ReviewMapClassification::ReviewRequired => &mut summary.review_required,
            ReviewMapClassification::Foldable => &mut summary.foldable,
            ReviewMapClassification::Unknown => &mut summary.unknown,
        };
        category.functions += 1;
        category.lines += region.line_count;
    }
    summary.must_review_lines = summary.review_required.lines;
    summary.low_semantic_risk_lines = summary.foldable.lines;
    summary.unknown_lines = summary.unknown.lines;
    summary.suggested_review_lines = summary.review_required.lines + summary.unknown.lines;
    summary.review_ratio =
        ReviewRatio::from_parts(summary.suggested_review_lines, summary.total_lines);
    summary.unknown_ratio = ReviewRatio::from_parts(summary.unknown.lines, summary.total_lines);
    summary.unknown_function_ratio =
        ReviewRatio::from_parts(summary.unknown.functions, summary.total_functions);
    summary
}

pub(super) fn review_map_modules(sources: &[ReviewMapParsedSource]) -> Vec<ReviewMapModule> {
    let mut modules = Vec::new();
    for source in sources {
        let uses = source
            .program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Use(use_decl) => Some(ReviewMapUse {
                    path: use_decl.path.join("."),
                    line: use_decl.span.line,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        for item in &source.program.items {
            if let Item::Module(module) = item {
                modules.push(ReviewMapModule {
                    file: source.file.clone(),
                    module_path: module.path.join("."),
                    line: module.span.line,
                    uses: uses.clone(),
                });
            }
        }
    }
    modules
}

#[derive(Debug, Clone)]
pub(super) struct ReviewMapParsedSource {
    file: String,
    total_lines: usize,
    program: Program,
}

pub(super) fn review_map_file_region_drafts(
    source: &ReviewMapParsedSource,
    hir: &Hir,
) -> Vec<ReviewMapRegionDraft> {
    let mut function_lines = source
        .program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function.span.line)),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect::<Vec<_>>();
    function_lines.sort_by_key(|(_, line)| *line);

    source
        .program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(review_map_region_draft(
                &source.file,
                function,
                &hir,
                &function_lines,
                source.total_lines,
            )),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect()
}

pub(super) fn review_map_region_draft(
    file: &str,
    function: &FunctionDecl,
    hir: &Hir,
    function_lines: &[(String, usize)],
    total_lines: usize,
) -> ReviewMapRegionDraft {
    let mut facts = ReviewMapFacts::default();
    // Build value_types for receiver-call resolution
    for param in &function.params {
        facts
            .value_types
            .insert(param.name.clone(), type_ref_display_name(&param.ty));
    }
    for type_param in &function.type_params {
        if let Some(GenericBound::Protocol(protocol)) = &type_param.bound {
            facts.value_types.insert(
                format!("__protocol_bound__{}", type_param.name),
                protocol.clone(),
            );
        }
    }
    let callback_params = review_map_callback_params(function);
    let local_closure_bindings = review_map_local_closure_bindings(&function.body);
    collect_review_map_facts_block(
        &function.body,
        hir,
        &callback_params,
        &local_closure_bindings,
        &mut facts,
    );
    if let Some(hir_body) = hir.function_body(&function.name) {
        let local_bindings = hir_body
            .bindings
            .iter()
            .filter(|binding| binding.kind == HirBindingKind::LocalLet)
            .map(|binding| binding.name.as_str())
            .collect::<BTreeSet<_>>();
        if let Some(body) = hir_body.block.as_ref() {
            collect_review_map_hir_facts_block(body, &local_bindings, &mut facts);
        }
    }

    let mut reasons = Vec::new();
    if function.is_public {
        reasons.push("public entry point".to_string());
    }
    if let Some(reason) = &function.deprecated_reason {
        reasons.push(format!("deprecated: {reason}"));
    }
    if is_entry_function(&function.name) {
        reasons.push("entry point".to_string());
    }
    for param in &function.params {
        if matches!(param.effect, Some(DataEffect::Mut | DataEffect::Take)) {
            reasons.push(format!(
                "{} parameter `{}`",
                effect_label(param.effect.expect("effect matched")),
                param.name
            ));
        }
        if type_ref_contains_name(&param.ty, "ResourcePool") {
            reasons.push(format!("ResourcePool parameter `{}`", param.name));
        }
    }
    if let Some(return_ty) = &function.return_ty
        && type_ref_contains_name(return_ty, "ResourcePool")
    {
        reasons.push("ResourcePool return type".to_string());
    }
    if function.returns_fresh {
        reasons.push("fresh guarantee boundary".to_string());
    }
    if function.is_async {
        reasons.push("async function boundary".to_string());
    }
    for effect in &function.effects {
        match effect {
            EffectDecl::Retains(param) => reasons.push(format!("retains `{param}`")),
            EffectDecl::Name(name) if matches!(name.as_str(), "native" | "unsafe" | "parallel") => {
                reasons.push(format!("{name} boundary"))
            }
            EffectDecl::Name(name) if is_runtime_guarantee_boundary(name) => {
                reasons.push(format!("guarantee `{name}`"))
            }
            _ => {}
        }
    }
    for call in &facts.native_calls {
        reasons.push(format!("native call `{call}`"));
    }
    for call in &facts.unsafe_calls {
        reasons.push(format!("unsafe call `{call}`"));
    }
    if facts.has_local {
        reasons.push("local binding".to_string());
    }
    if facts.has_manage {
        reasons.push("manage boundary".to_string());
    }
    if facts.has_spawn {
        reasons.push("spawn task boundary".to_string());
    }
    if facts.has_await {
        reasons.push("await suspension boundary".to_string());
    }
    if !facts.spawn_captures.is_empty() {
        let captures = facts
            .spawn_captures
            .iter()
            .map(|capture| format!("`{capture}`"))
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("spawn retains-until-task-complete {captures}"));
    }
    if !facts.managed_closure_captures.is_empty() {
        let captures = facts
            .managed_closure_captures
            .iter()
            .map(|capture| format!("`{capture}`"))
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("managed closure retains {captures}"));
    }
    for contract in &facts.explicit_closure_contracts {
        reasons.push(format!("explicit closure {contract}"));
    }
    if facts.has_with {
        reasons.push("resource with block".to_string());
    }
    if facts.has_take {
        reasons.push("take effect".to_string());
    }
    if facts.has_mut {
        reasons.push("mut call-site effect".to_string());
    }
    if facts.has_resource_pool {
        reasons.push("ResourcePool usage".to_string());
    }
    if facts.has_handle_field_write {
        reasons.push("writes through handle field".to_string());
    }
    if facts.has_managed_state_write {
        reasons.push("writes to managed state".to_string());
    }
    if facts.has_error_boundary {
        reasons.push("error handling boundary".to_string());
    }
    if facts.has_capability_object {
        reasons.push("capability object construction".to_string());
    }
    if facts.has_dynamic_protocol_dispatch {
        reasons.push("dynamic protocol dispatch".to_string());
    }
    for callback in &facts.callback_calls {
        reasons.push(format!("noescape callback call `{callback}`"));
    }

    let classification = if !facts.unresolved_calls.is_empty() {
        let calls = facts
            .unresolved_calls
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("unresolved call(s): {calls}"));
        ReviewMapClassification::Unknown
    } else if reasons.is_empty() {
        reasons.push("private pure helper with no retention or resource boundary".to_string());
        ReviewMapClassification::Foldable
    } else {
        ReviewMapClassification::ReviewRequired
    };

    ReviewMapRegionDraft {
        file: file.to_string(),
        region: ReviewMapRegion {
            function: function.name.clone(),
            classification,
            line: function.span.line,
            line_count: review_map_line_count(
                &function.name,
                function.span.line,
                function_lines,
                total_lines,
            ),
            reasons,
            receiver_calls: facts.receiver_calls.clone(),
        },
        facts,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReviewMapRegionDraft {
    file: String,
    region: ReviewMapRegion,
    facts: ReviewMapFacts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReviewMapFacts {
    pub(super) has_local: bool,
    pub(super) has_manage: bool,
    pub(super) has_spawn: bool,
    pub(super) has_await: bool,
    pub(super) has_with: bool,
    pub(super) has_mut: bool,
    pub(super) has_take: bool,
    pub(super) has_resource_pool: bool,
    pub(super) has_handle_field_write: bool,
    pub(super) has_managed_state_write: bool,
    pub(super) has_error_boundary: bool,
    pub(super) has_capability_object: bool,
    pub(super) has_dynamic_protocol_dispatch: bool,
    pub(super) user_calls: BTreeSet<String>,
    pub(super) unresolved_calls: BTreeSet<String>,
    pub(super) callback_calls: BTreeSet<String>,
    pub(super) receiver_calls: Vec<ReviewMapReceiverCall>,
    pub(super) native_calls: BTreeSet<String>,
    pub(super) unsafe_calls: BTreeSet<String>,
    pub(super) spawn_captures: BTreeSet<String>,
    pub(super) managed_closure_captures: BTreeSet<String>,
    pub(super) explicit_closure_contracts: BTreeSet<String>,
    /// Value types for receiver-call resolution (param_name -> type_name,
    /// plus `__protocol_bound__<T>` -> protocol for generic bounds).
    pub(super) value_types: HashMap<String, String>,
}

pub(super) fn propagate_review_map_call_classifications(drafts: &mut [ReviewMapRegionDraft]) {
    propagate_unknown_calls(drafts);
    propagate_review_required_calls(drafts);
}

pub(super) fn propagate_unknown_calls(drafts: &mut [ReviewMapRegionDraft]) {
    loop {
        let classifications = drafts
            .iter()
            .map(|draft| (draft.region.function.clone(), draft.region.classification))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;

        for draft in drafts.iter_mut() {
            if draft.region.classification == ReviewMapClassification::Unknown {
                continue;
            }
            let Some(callee) = draft.facts.user_calls.iter().find(|callee| {
                classifications.get(*callee) == Some(&ReviewMapClassification::Unknown)
            }) else {
                continue;
            };
            draft.region.classification = ReviewMapClassification::Unknown;
            draft.region.reasons.retain(|reason| {
                reason != "private pure helper with no retention or resource boundary"
            });
            draft
                .region
                .reasons
                .push(format!("calls unknown `{callee}`"));
            changed = true;
        }

        if !changed {
            break;
        }
    }
}

pub(super) fn propagate_review_required_calls(drafts: &mut [ReviewMapRegionDraft]) {
    loop {
        let classifications = drafts
            .iter()
            .map(|draft| (draft.region.function.clone(), draft.region.classification))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;

        for draft in drafts.iter_mut() {
            if draft.region.classification != ReviewMapClassification::Foldable {
                continue;
            }
            let Some(callee) = draft.facts.user_calls.iter().find(|callee| {
                classifications.get(*callee) == Some(&ReviewMapClassification::ReviewRequired)
            }) else {
                continue;
            };
            draft.region.classification = ReviewMapClassification::ReviewRequired;
            draft.region.reasons.retain(|reason| {
                reason != "private pure helper with no retention or resource boundary"
            });
            draft
                .region
                .reasons
                .push(format!("calls must-review `{callee}`"));
            changed = true;
        }

        if !changed {
            break;
        }
    }
}

pub(super) fn receiver_call_resolution_label(resolution: &CallResolution) -> &'static str {
    match resolution {
        CallResolution::Resolved { kind, .. } => match kind {
            ResolvedCalleeKind::UserFunction => "user_function",
            ResolvedCalleeKind::BuiltinFunction => "builtin_function",
            ResolvedCalleeKind::Constructor { .. } => "constructor",
        },
        CallResolution::EnumVariant => "enum_variant",
        CallResolution::Ambiguous { .. } => "ambiguous",
        CallResolution::Unknown => "unknown",
    }
}

pub(super) fn review_map_callback_params(function: &FunctionDecl) -> BTreeSet<String> {
    function
        .params
        .iter()
        .filter(|param| param.ty.is_noescape && param.ty.name == "Fn")
        .map(|param| param.name.clone())
        .collect()
}

pub(super) fn review_map_local_closure_bindings(block: &Block) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    collect_review_map_local_closure_bindings_block(block, &mut bindings);
    bindings
}

pub(super) fn review_callee_display(callee: &Callee) -> String {
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
            review_expr_label(receiver)
        ),
    }
}

pub(super) fn review_map_expr_type_name_with_facts(
    expr: &Expr,
    hir: &Hir,
    value_types: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => value_types
            .get(name)
            .cloned()
            .or_else(|| hir.sum_type_for_variant(name).map(str::to_string)),
        Expr::Call {
            callee: Callee::ReceiverCall {
                receiver, method, ..
            },
            ..
        } => {
            let receiver_type = review_map_expr_type_name_with_facts(receiver, hir, value_types)?;
            match hir
                .resolve_receiver_call(&receiver_type, method, value_types)
                .0
            {
                CallResolution::Resolved { signature, .. } => signature.return_type,
                CallResolution::Ambiguous { .. }
                | CallResolution::EnumVariant
                | CallResolution::Unknown => None,
            }
        }
        Expr::Call { .. } => review_map_expr_type_name(expr, hir),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            review_map_expr_type_name_with_facts(value, hir, value_types)
        }
        Expr::Try { value, .. } => review_map_expr_type_name_with_facts(value, hir, value_types)
            .and_then(|ty| result_ok_type_name(&ty)),
        Expr::Await { value, .. } => review_map_expr_type_name_with_facts(value, hir, value_types),
        Expr::Field { base, name, .. } => {
            let base_type = review_map_expr_type_name_with_facts(base, hir, value_types)?;
            hir.type_info(&base_type)
                .and_then(|info| info.fields.get(name))
                .map(|field| field.type_name.clone())
        }
        _ => review_map_expr_type_name(expr, hir),
    }
}

pub(super) fn review_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::CharLiteral(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Field { base, name, .. } => format!("{}.{}", review_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", review_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", review_callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            review_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

pub(super) fn is_resource_pool_callee(callee: &Callee) -> bool {
    match callee {
        Callee::Name(name) => type_root_name(name) == "ResourcePool",
        Callee::Qualified { namespace, .. } => type_root_name(namespace) == "ResourcePool",
        Callee::ReceiverCall { .. } => false,
    }
}

pub(super) fn is_capability_from_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Capability" && type_root_name(name) == "from"
    )
}

pub(super) fn is_capability_protocol_call(
    callee: &Callee,
    args: &[CallArg],
    value_types: &HashMap<String, String>,
) -> bool {
    let Callee::Qualified { namespace, .. } = callee else {
        return false;
    };
    let Some(self_arg) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("self"))
        .or_else(|| args.first())
    else {
        return false;
    };
    expr_ident_name(&self_arg.value)
        .and_then(|name| value_types.get(name))
        .and_then(|type_name| capability_protocol_name(type_name))
        .is_some_and(|protocol| protocol == namespace)
}

pub(super) fn expr_ident_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => expr_ident_name(value),
        _ => None,
    }
}

pub(super) fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
}

pub(super) fn review_map_match_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&str>,
) -> Option<(String, String)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.map(|ty| (name.clone(), ty.to_string()));
    }
    let MatchPattern::Variant { name, bindings, .. } = pattern else {
        return None;
    };
    // Option/Result carry a single positional payload.
    let Some(binding) = bindings.first() else {
        return None;
    };
    let value_type = value_type?;
    let args = type_arg_names(value_type)?;
    match name.as_str() {
        "Some" if type_root_name(value_type) == "Option" => args
            .first()
            .and_then(|ty| review_map_match_binding_type(binding, Some(ty))),
        "Ok" if type_root_name(value_type) == "Result" => args
            .first()
            .and_then(|ty| review_map_match_binding_type(binding, Some(ty))),
        "Err" if type_root_name(value_type) == "Result" => args
            .get(1)
            .and_then(|ty| review_map_match_binding_type(binding, Some(ty))),
        _ => None,
    }
}

pub(super) fn type_ref_display_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }
    format!(
        "{}<{}>",
        ty.name,
        ty.args
            .iter()
            .map(type_ref_display_name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn result_ok_type_name(type_name: &str) -> Option<String> {
    if type_root_name(type_name) != "Result" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().map(|ty| (*ty).to_string()))
}

pub(super) fn capability_protocol_name(type_name: &str) -> Option<&str> {
    if type_root_name(type_name) != "Capability" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().copied())
}

pub(super) fn review_map_line_count(
    function_name: &str,
    start_line: usize,
    function_lines: &[(String, usize)],
    total_lines: usize,
) -> usize {
    let next_line = function_lines
        .iter()
        .find(|(name, line)| name != function_name && *line > start_line)
        .map(|(_, line)| *line)
        .unwrap_or(total_lines + 1);
    next_line.saturating_sub(start_line).max(1)
}

pub(super) fn review_map_classification_label(
    classification: ReviewMapClassification,
) -> &'static str {
    match classification {
        ReviewMapClassification::ReviewRequired => "must-review",
        ReviewMapClassification::Foldable => "low-semantic-risk",
        ReviewMapClassification::Unknown => "unknown",
    }
}

pub(super) fn review_map_file_risk_label(risk: ReviewMapFileRisk) -> &'static str {
    match risk {
        ReviewMapFileRisk::Low => "low",
        ReviewMapFileRisk::Elevated => "elevated",
        ReviewMapFileRisk::High => "high",
    }
}

pub(super) fn is_entry_function(name: &str) -> bool {
    matches!(name, "main" | "run")
        || name.starts_with("run_")
        || name.starts_with("handle_")
        || name.ends_with("_handler")
}

pub(super) fn is_runtime_guarantee_boundary(effect: &str) -> bool {
    matches!(effect, "no_panic" | "noalloc" | "no_block" | "pure")
}

pub(super) fn feature_label(features: &[FileFeature]) -> String {
    let labels = feature_names(features);
    if labels.is_empty() {
        return "<none>".to_string();
    }
    labels.join(",")
}

pub(super) fn feature_names(features: &[FileFeature]) -> Vec<String> {
    let mut labels = features
        .iter()
        .map(feature_name)
        .map(str::to_string)
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    labels
}

pub(super) fn feature_name(feature: &FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}

pub(super) fn review_map_file_risk(features: &[FileFeature]) -> ReviewMapFileRisk {
    if features.iter().any(|feature| {
        matches!(
            feature,
            FileFeature::Native | FileFeature::Unsafe | FileFeature::Device | FileFeature::Ffi
        )
    }) {
        ReviewMapFileRisk::High
    } else if features.iter().any(|feature| {
        matches!(
            feature,
            FileFeature::Local | FileFeature::Async | FileFeature::Reflection
        )
    }) {
        ReviewMapFileRisk::Elevated
    } else {
        ReviewMapFileRisk::Low
    }
}

pub(super) fn review_map_file_reasons(features: &[FileFeature]) -> Vec<String> {
    feature_names(features)
        .into_iter()
        .filter_map(|feature| review_map_feature_reason(&feature).map(str::to_string))
        .collect()
}

pub(super) fn review_map_feature_reason(feature: &str) -> Option<&'static str> {
    match feature {
        "local" => Some("local capability enabled"),
        "native" => Some("native boundary capability enabled"),
        "unsafe" => Some("unsafe capability enabled"),
        "async" => Some("async control-flow capability enabled"),
        "device" => Some("reserved device review marker enabled"),
        "ffi" => Some("reserved ffi review marker enabled"),
        "reflection" => Some("reserved reflection review marker enabled"),
        _ => None,
    }
}
