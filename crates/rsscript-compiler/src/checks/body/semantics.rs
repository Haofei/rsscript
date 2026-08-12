use super::*;
use crate::checks::diagnostic_helpers::error_cause_manual_fix;

pub(super) fn check_stmt_semantics(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    statement: &HirStmt,
    state: &mut BodyState,
    check_resource_contexts: bool,
    live_after: &HashSet<String>,
) -> Flow {
    match statement {
        HirStmt::Let {
            kind,
            value,
            is_async,
            span,
            ..
        } => {
            let mut merged_entry_state;
            let stmt_state = if let Some(entry_state) = local_analysis.flow_entry_state(span) {
                merged_entry_state = entry_state.clone();
                merged_entry_state
                    .read_views
                    .extend(state.read_views.iter().cloned());
                &merged_entry_state
            } else {
                state
            };
            if *kind == HirBindingKind::ManagedLet {
                check_managed_closure_captures(analyzer, local_analysis, span, stmt_state);
            }
            if let Some(value) = value {
                if *is_async {
                    // async let consumes the async call (produces a pending)
                    check_expr_semantics_with_context(
                        analyzer,
                        local_analysis,
                        value,
                        stmt_state,
                        false,
                        true,
                        live_after,
                    );
                } else {
                    check_expr_semantics(analyzer, local_analysis, value, stmt_state, live_after);
                }
                if check_resource_contexts {
                    check_resource_producer_expr(analyzer, value, false);
                }
            }

            Flow::Fallthrough
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_expr_semantics(analyzer, local_analysis, value, state, live_after);
                if check_resource_contexts {
                    check_resource_producer_expr(analyzer, value, false);
                }
            }
            Flow::Return
        }
        HirStmt::With {
            resource,
            body,
            span,
            binding,
            ..
        } => {
            check_expr_semantics(analyzer, local_analysis, resource, state, live_after);
            if check_resource_contexts {
                check_result_resource_with_has_try(analyzer, resource);
                check_resource_producer_expr(analyzer, resource, true);
                check_resource_escape(analyzer, local_analysis, span);
            }
            let mut scoped_state = state.clone();
            scoped_state.bind_resource(binding.clone());
            check_block(
                analyzer,
                local_analysis,
                body,
                &mut scoped_state,
                check_resource_contexts,
                live_after,
            )
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            if let Some(diagnostic) = rsscript_semantics::bool_condition_diagnostic(condition, "if")
            {
                analyzer.diagnostics.push(diagnostic);
            }
            check_expr_semantics(analyzer, local_analysis, condition, state, live_after);
            if check_resource_contexts {
                check_resource_producer_expr(analyzer, condition, false);
            }
            apply_expr_effects(condition, state);

            let base_state = state.clone();
            let mut then_state = base_state.clone();
            let then_flow = check_block(
                analyzer,
                local_analysis,
                then_body,
                &mut then_state,
                check_resource_contexts,
                live_after,
            );

            let else_branch = else_body.as_ref().map(|else_body| {
                let mut else_state = base_state.clone();
                let else_flow = check_block(
                    analyzer,
                    local_analysis,
                    else_body,
                    &mut else_state,
                    check_resource_contexts,
                    live_after,
                );
                (else_state, else_flow)
            });

            merge_if_state(state, &base_state, then_state, then_flow, else_branch)
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                if let Some(diagnostic) =
                    rsscript_semantics::bool_condition_diagnostic(condition, "while")
                {
                    analyzer.diagnostics.push(diagnostic);
                }
                check_expr_semantics(analyzer, local_analysis, condition, state, live_after);
                if check_resource_contexts {
                    check_resource_producer_expr(analyzer, condition, false);
                }
                apply_expr_effects(condition, state);
            }

            let base_state = state.clone();
            let mut body_state = base_state.clone();
            let body_flow = check_block(
                analyzer,
                local_analysis,
                body,
                &mut body_state,
                check_resource_contexts,
                live_after,
            );

            merge_loop_state(
                state,
                &base_state,
                body_state,
                body_flow,
                condition.is_some(),
            )
        }
        HirStmt::For {
            binding,
            iterable,
            iterable_type_name,
            item_type_name,
            is_async,
            body,
            ..
        } => {
            if let Some(diagnostic) = rsscript_semantics::for_iterable_diagnostic(
                iterable,
                iterable_type_name.as_deref(),
                *is_async,
            ) {
                analyzer.diagnostics.push(diagnostic);
            }
            check_expr_semantics(analyzer, local_analysis, iterable, state, live_after);
            if check_resource_contexts {
                check_resource_producer_expr(analyzer, iterable, false);
            }
            apply_expr_effects(iterable, state);

            let base_state = state.clone();
            let mut body_state = base_state.clone();
            if let Some(item_type_name) = item_type_name {
                body_state.record_type(binding.clone(), item_type_name.clone());
                if analyzer.hir.type_kind(type_root_name(item_type_name))
                    == Some(HirTypeKind::Class)
                {
                    body_state.bind_managed(binding.clone());
                } else if !is_async && !is_copy_type_name(item_type_name) {
                    body_state.bind_read_view(binding.clone());
                }
            }
            let body_flow = check_block(
                analyzer,
                local_analysis,
                body,
                &mut body_state,
                check_resource_contexts,
                live_after,
            );
            merge_loop_state(state, &base_state, body_state, body_flow, true)
        }
        HirStmt::Match {
            value,
            scrutinee_effect,
            arms,
            ..
        } => {
            check_match_scrutinee_type(analyzer, value);
            check_match_patterns_match_scrutinee(analyzer, value, arms);
            check_match_pattern_effects(analyzer, value, *scrutinee_effect, arms);
            if *scrutinee_effect == Some(DataEffect::Take) {
                check_take_operand_is_local(analyzer, value, &arm_span(arms), state);
            }
            check_expr_semantics(analyzer, local_analysis, value, state, live_after);
            if check_resource_contexts {
                check_resource_producer_expr(analyzer, value, false);
            }
            apply_expr_effects(value, state);
            apply_match_scrutinee_effect(*scrutinee_effect, value, &arm_span(arms), state);

            let base_state = state.clone();
            let mut all_return = !arms.is_empty();
            for arm in arms {
                let mut arm_state = base_state.clone();
                let flow = check_block(
                    analyzer,
                    local_analysis,
                    &arm.body,
                    &mut arm_state,
                    check_resource_contexts,
                    live_after,
                );
                all_return &= flow == Flow::Return;
            }
            if all_return {
                Flow::Return
            } else {
                Flow::Fallthrough
            }
        }
        HirStmt::Select { arms, .. } => {
            // Every arm operation is constructed and polled, so their effects
            // apply to the shared state first. Then exactly one body runs, so the
            // bodies are mutually-exclusive branches off that common base — like
            // `match` arms — which avoids false cross-arm ownership conflicts.
            for arm in arms {
                check_expr_semantics(analyzer, local_analysis, &arm.operation, state, live_after);
                if check_resource_contexts {
                    check_resource_producer_expr(analyzer, &arm.operation, false);
                }
                apply_expr_effects(&arm.operation, state);
            }
            let base_state = state.clone();
            let mut all_return = !arms.is_empty();
            for arm in arms {
                let mut arm_state = base_state.clone();
                if arm.binding != "_" {
                    arm_state.bind_local(arm.binding.clone());
                }
                let flow = check_block(
                    analyzer,
                    local_analysis,
                    &arm.body,
                    &mut arm_state,
                    check_resource_contexts,
                    live_after,
                );
                all_return &= flow == Flow::Return;
            }
            if all_return {
                Flow::Return
            } else {
                Flow::Fallthrough
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
            check_expr_semantics(analyzer, local_analysis, expr, state, live_after);
            if check_resource_contexts {
                check_resource_producer_expr(analyzer, expr, false);
            }
            Flow::Fallthrough
        }
        HirStmt::Break(_) => Flow::Break,
        HirStmt::Continue(_) => Flow::Continue,
        HirStmt::Unknown(_) => Flow::Fallthrough,
    }
}

pub(super) fn apply_stmt_effects(statement: &HirStmt, state: &mut BodyState) {
    match statement {
        HirStmt::Let {
            kind,
            name,
            value,
            ty,
            ..
        } => {
            match kind {
                HirBindingKind::ManagedLet => state.bind_managed(name.clone()),
                HirBindingKind::LocalLet => state.bind_local(name.clone()),
                HirBindingKind::Param => {}
            }
            if let Some(ty) = ty {
                state.record_type(name.clone(), ty.to_string());
            }
            if let Some(value) = value {
                apply_expr_effects(value, state);
            }
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                apply_expr_effects(value, state);
            }
        }
        HirStmt::With { resource, .. } => {
            apply_expr_effects(resource, state);
        }
        HirStmt::If { .. } => {}
        HirStmt::Loop { .. } => {}
        HirStmt::For { iterable, .. } => apply_expr_effects(iterable, state),
        HirStmt::Match { value, .. } => apply_expr_effects(value, state),
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                apply_expr_effects(&arm.operation, state);
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
            apply_expr_effects(expr, state)
        }
        HirStmt::Break(_) | HirStmt::Continue(_) => {}
        HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_expr_semantics(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    expr: &HirExpr,
    state: &BodyState,
    live_after: &HashSet<String>,
) {
    check_expr_semantics_with_context(
        analyzer,
        local_analysis,
        expr,
        state,
        false,
        false,
        live_after,
    );
}

pub(super) fn check_match_scrutinee_type(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    let type_name = analyzer.expand_type_alias(type_name);
    let is_declared_pattern_type = matches!(
        analyzer.hir.type_kind(&type_name),
        Some(HirTypeKind::Sum | HirTypeKind::Struct | HirTypeKind::Class)
    );
    if let Some(diagnostic) = rsscript_semantics::match_scrutinee_diagnostic(
        expr,
        Some(&type_name),
        is_declared_pattern_type,
    ) {
        analyzer.diagnostics.push(diagnostic);
    }
}

pub(super) fn check_match_patterns_match_scrutinee(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    arms: &[HirMatchArm],
) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    for arm in arms {
        check_match_pattern_matches_type(analyzer, &arm.pattern, type_name);
    }
}

pub(super) fn check_match_pattern_matches_type(
    analyzer: &mut Analyzer<'_>,
    pattern: &MatchPattern,
    type_name: &str,
) {
    let type_name = analyzer.expand_type_alias(type_name);
    let root = type_root_name(&type_name);
    match pattern {
        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => {}
        MatchPattern::Literal { value, span } => {
            if let MatchLiteral::Char(raw) = value
                && let Some(diagnostic) =
                    rsscript_semantics::match_char_literal_scalar_diagnostic(raw, span)
            {
                analyzer.diagnostics.push(diagnostic);
            }
            if let Some(diagnostic) =
                rsscript_semantics::match_literal_type_diagnostic(value, span, &type_name)
            {
                analyzer.diagnostics.push(diagnostic);
            }
        }
        MatchPattern::Variant {
            name,
            bindings,
            span,
            ..
        } if root == "Option" => {
            if !matches!(name.as_str(), "Some" | "None") {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::match_variant_family_diagnostic(
                        name,
                        &type_name,
                        &["Some".to_string(), "None".to_string()],
                        span,
                    ));
                return;
            }
            // `Some` carries one payload, `None` carries none.
            let expected = if name == "Some" { 1 } else { 0 };
            if !bindings.is_empty() && bindings.len() != expected {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::variant_pattern_arity_diagnostic(
                        name,
                        expected,
                        bindings.len(),
                        span,
                    ));
                return;
            }
            if name == "Some"
                && let Some(binding) = bindings.first()
                && let Some(inner) =
                    type_arg_names(&type_name).and_then(|args| args.first().copied())
            {
                check_match_pattern_matches_type(analyzer, binding, inner);
            }
        }
        MatchPattern::Variant {
            name,
            bindings,
            span,
            ..
        } if root == "Result" => {
            if !matches!(name.as_str(), "Ok" | "Err") {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::match_variant_family_diagnostic(
                        name,
                        &type_name,
                        &["Ok".to_string(), "Err".to_string()],
                        span,
                    ));
                return;
            }
            if !bindings.is_empty() && bindings.len() != 1 {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::variant_pattern_arity_diagnostic(
                        name,
                        1,
                        bindings.len(),
                        span,
                    ));
                return;
            }
            if let Some(binding) = bindings.first()
                && let Some(args) = type_arg_names(&type_name)
            {
                let payload_type = if name == "Ok" {
                    args.first()
                } else {
                    args.get(1)
                };
                if let Some(payload_type) = payload_type {
                    check_match_pattern_matches_type(analyzer, binding, payload_type);
                }
            }
        }
        MatchPattern::Variant {
            name,
            bindings,
            span,
            ..
        } => {
            let Some((_, fields)) = pattern_sum_variant_fields(analyzer, root, name) else {
                let allowed = allowed_sum_variant_names(analyzer, root);
                if allowed.is_empty() {
                    analyzer
                        .diagnostics
                        .push(rsscript_semantics::match_pattern_type_diagnostic(
                            name, &type_name, span,
                        ));
                } else {
                    analyzer
                        .diagnostics
                        .push(rsscript_semantics::match_variant_family_diagnostic(
                            name, &type_name, &allowed, span,
                        ));
                }
                return;
            };
            // A bare variant name (`V`) matches without binding any field. Once a
            // positional payload is written, its arity must equal the declared
            // field count, and each sub-pattern is checked against the field type
            // at the same position (the RS0037 safety net for positional binding).
            if !bindings.is_empty() && bindings.len() != fields.len() {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::variant_pattern_arity_diagnostic(
                        name,
                        fields.len(),
                        bindings.len(),
                        span,
                    ));
                return;
            }
            for (binding, field) in bindings.iter().zip(fields.iter()) {
                check_match_pattern_matches_type(analyzer, binding, &field.ty.to_string());
            }
        }
        MatchPattern::Struct {
            name, fields, span, ..
        } => {
            let declared = if name == root
                && matches!(
                    analyzer.hir.type_kind(root),
                    Some(HirTypeKind::Struct | HirTypeKind::Class)
                ) {
                analyzer
                    .hir
                    .type_info(root)
                    .map(|info| info.fields.values().cloned().collect::<Vec<FieldInfo>>())
            } else {
                pattern_sum_variant_fields(analyzer, root, name).map(|(_, fields)| fields)
            };
            let Some(declared) = declared else {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::match_pattern_type_diagnostic(
                        name, &type_name, span,
                    ));
                return;
            };
            // Map the type's declared parameters (`A`, `B`) to the scrutinee's
            // concrete arguments so a field declared `A` is checked as `Int`.
            let type_params = analyzer
                .hir
                .type_info(root)
                .map(|info| info.type_params.to_vec())
                .unwrap_or_default();
            let substitutions = generic_substitutions(&type_params, &type_name);
            for field in fields {
                if let Some(pattern) = &field.pattern
                    && let Some(field_info) = declared
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                {
                    let field_type =
                        substitute_type_args(&field_info.ty.to_string(), &substitutions);
                    check_match_pattern_matches_type(analyzer, pattern, &field_type);
                }
            }
        }
        MatchPattern::List {
            prefix,
            suffix,
            span,
            ..
        } => {
            if root != "List" {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::match_pattern_type_diagnostic(
                        "[..]", &type_name, span,
                    ));
                return;
            }
            // Each element pattern is checked against the list's element type `T`
            // (`List<T>`); the rest binding (if any) is itself a `List<T>`.
            if let Some(element_type) =
                type_arg_names(&type_name).and_then(|args| args.first().copied())
            {
                for pattern in prefix.iter().chain(suffix) {
                    check_match_pattern_matches_type(analyzer, pattern, element_type);
                }
            }
        }
    }
}

pub(super) fn allowed_sum_variant_names(analyzer: &Analyzer<'_>, root: &str) -> Vec<String> {
    match root {
        "Option" => return vec!["Some".to_string(), "None".to_string()],
        "Result" => return vec!["Ok".to_string(), "Err".to_string()],
        _ => {}
    }
    analyzer
        .syntax_program
        .items
        .iter()
        .find_map(|item| match item {
            Item::SumType(sum) if sum.name == root => Some(
                sum.variants
                    .iter()
                    .map(|variant| variant.name.clone())
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// Build a map from a type's declared generic parameters to the concrete
/// arguments named in `type_name`, e.g. params `[A, B]` against
/// `__Tuple2<Int, String>` → `{A: Int, B: String}`.
pub(super) fn generic_substitutions(
    type_params: &[String],
    type_name: &str,
) -> HashMap<String, String> {
    let Some(args) = type_arg_names(type_name) else {
        return HashMap::new();
    };
    type_params
        .iter()
        .zip(args)
        .map(|(param, arg)| (param.clone(), arg.to_string()))
        .collect()
}

pub(super) fn pattern_sum_variant_fields(
    analyzer: &Analyzer<'_>,
    root: &str,
    variant_name: &str,
) -> Option<(String, Vec<FieldInfo>)> {
    analyzer
        .hir
        .sum_type_for_variant(variant_name)
        .filter(|sum| *sum == root)?;
    analyzer
        .hir
        .sum_variant_fields(variant_name)
        .map(<[FieldInfo]>::to_vec)
        .map(|fields| (root.to_string(), fields))
}

pub(super) fn check_match_pattern_effects(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    scrutinee_effect: Option<DataEffect>,
    arms: &[HirMatchArm],
) {
    let scrutinee_type = hir_expr_type_name(value).map(|ty| analyzer.expand_type_alias(ty));
    let managed_class_scrutinee = hir_expr_type_name(value)
        .map(|ty| analyzer.expand_type_alias(ty))
        .as_deref()
        .map(type_root_name)
        .is_some_and(|root| analyzer.hir.type_kind(root) == Some(HirTypeKind::Class));
    for arm in arms {
        if let Some(diagnostic) = rsscript_semantics::structured_match_effect_diagnostic(
            &arm.pattern,
            scrutinee_effect,
            &arm.span,
        ) {
            analyzer.diagnostics.push(diagnostic);
        }
        check_pattern_field_effects(
            analyzer,
            scrutinee_type.as_deref(),
            scrutinee_effect,
            managed_class_scrutinee,
            &arm.pattern,
        );
        if let Some(guard) = &arm.guard
            && let Some((effect, span)) = first_mutating_effect_expr(guard)
        {
            analyzer
                .diagnostics
                .push(rsscript_semantics::match_guard_mutation_diagnostic(
                    effect, span,
                ));
        }
    }
}

pub(super) fn check_pattern_field_effects(
    analyzer: &mut Analyzer<'_>,
    scrutinee_type: Option<&str>,
    scrutinee_effect: Option<DataEffect>,
    managed_class_scrutinee: bool,
    pattern: &MatchPattern,
) {
    let MatchPattern::Struct {
        name,
        fields,
        has_rest,
        span,
        ..
    } = pattern
    else {
        return;
    };
    check_struct_pattern_fields(analyzer, scrutinee_type, name, fields, *has_rest, span);
    let mut seen_fields: HashMap<&str, (DataEffect, Span)> = HashMap::new();
    for field in fields {
        let effective_effect = field.effect.unwrap_or(match scrutinee_effect {
            Some(DataEffect::Take) => DataEffect::Take,
            _ => DataEffect::Read,
        });
        if let Some((previous_effect, previous_span)) =
            seen_fields.insert(field.name.as_str(), (effective_effect, field.span.clone()))
        {
            let conflicts = matches!(previous_effect, DataEffect::Mut | DataEffect::Take)
                || matches!(effective_effect, DataEffect::Mut | DataEffect::Take);
            if conflicts {
                analyzer.diagnostics.push(error_cause_manual_fix(
                    code::FIELD_PARTIAL_ACCESS_CONFLICT,
                    format!(
                        "pattern field `{}` is bound more than once with mutable or taking access.",
                        field.name
                    ),
                    field.span.clone(),
                    "pattern field conflict",
                    format!(
                        "The previous binding for `{}` was at {}:{}.",
                        field.name, previous_span.line, previous_span.column
                    ),
                    "remove_overlapping_pattern_binding",
                    "Bind each mutable or taking field place at most once in a pattern.",
                ));
            }
        }
        if field.ignored {
            continue;
        };
        let effect = effective_effect;
        if managed_class_scrutinee && matches!(effect, DataEffect::Mut | DataEffect::Take) {
            let diagnostic = rsscript_semantics::managed_pattern_field_effect_diagnostic(
                &field.name,
                effect,
                &field.span,
            )
            .expect("mutating managed pattern fields must produce a diagnostic");
            analyzer.diagnostics.push(diagnostic);
            continue;
        }
        let allowed = match scrutinee_effect {
            Some(DataEffect::Read) => effect == DataEffect::Read,
            Some(DataEffect::Mut) => matches!(effect, DataEffect::Read | DataEffect::Mut),
            Some(DataEffect::Take) => true,
            None => effect == DataEffect::Read,
        };
        if !allowed {
            analyzer.diagnostics.push(
                rsscript_semantics::weakened_pattern_field_effect_diagnostic(
                    &field.name,
                    effect,
                    &field.span,
                ),
            );
        }
    }
}

pub(super) fn check_struct_pattern_fields(
    analyzer: &mut Analyzer<'_>,
    scrutinee_type: Option<&str>,
    pattern_name: &str,
    fields: &[MatchFieldPattern],
    has_rest: bool,
    pattern_span: &Span,
) {
    let Some(declared_fields) = declared_pattern_fields(analyzer, scrutinee_type, pattern_name)
    else {
        return;
    };
    let declared_names: HashSet<&str> = declared_fields.iter().map(String::as_str).collect();
    let mut seen_fields: HashMap<&str, Span> = HashMap::new();
    for field in fields {
        if let Some(previous_span) = seen_fields.insert(field.name.as_str(), field.span.clone()) {
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::FIELD_PARTIAL_ACCESS_CONFLICT,
                format!("pattern field `{}` is listed more than once.", field.name),
                field.span.clone(),
                "duplicate pattern field",
                format!(
                    "The previous projection of `{}` was at {}:{}.",
                    field.name, previous_span.line, previous_span.column
                ),
                "remove_duplicate_pattern_field",
                "List each field at most once in a structured pattern.",
            ));
        }
        if !declared_names.contains(field.name.as_str()) {
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::UNKNOWN_FIELD,
                format!("unknown field `{}` on type `{pattern_name}`.", field.name),
                field.span.clone(),
                "unknown field",
                "Structured match patterns may only project declared fields.",
                "use_declared_pattern_field",
                format!("Use a field declared on `{pattern_name}` or update the pattern."),
            ));
        }
    }
    if !has_rest && fields.len() < declared_fields.len() {
        analyzer.diagnostics.push(error_cause_manual_fix(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("pattern `{pattern_name} {{ ... }}` omits fields without `..`."),
            fields
                .last()
                .map(|field| field.span.clone())
                .unwrap_or_else(|| pattern_span.clone()),
            "pattern omits fields",
            "Omitted fields must be visible in review; write `..` when intentionally ignoring the rest.",
            "add_pattern_rest",
            format!("Write `{pattern_name} {{ ..., .. }}` when omitting fields."),
        ));
    }
}

pub(super) fn declared_pattern_fields(
    analyzer: &Analyzer<'_>,
    scrutinee_type: Option<&str>,
    pattern_name: &str,
) -> Option<Vec<String>> {
    let canonical_scrutinee = scrutinee_type.map(|ty| analyzer.expand_type_alias(ty))?;
    let scrutinee_root = type_root_name(&canonical_scrutinee);
    if analyzer
        .hir
        .sum_type_for_variant(pattern_name)
        .is_some_and(|sum| sum == scrutinee_root)
    {
        return analyzer
            .syntax_program
            .items
            .iter()
            .find_map(|item| match item {
                Item::SumType(sum) if sum.name == scrutinee_root => sum
                    .variants
                    .iter()
                    .find(|variant| variant.name == pattern_name)
                    .map(|variant| {
                        variant
                            .fields
                            .iter()
                            .map(|field| field.name.clone())
                            .collect()
                    }),
                _ => None,
            });
    }
    if pattern_name == scrutinee_root {
        return analyzer.hir.type_info(scrutinee_root).map(|type_info| {
            type_info
                .fields
                .values()
                .map(|field| field.name.clone())
                .collect()
        });
    }
    None
}

pub(super) fn first_mutating_effect_expr(
    expr: &HirExpr,
) -> Option<(DataEffect, &crate::diagnostic::Span)> {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Mut,
            span,
            ..
        } => Some((DataEffect::Mut, span)),
        HirExpr::Effect {
            effect: ParamEffect::Take,
            span,
            ..
        } => Some((DataEffect::Take, span)),
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => first_mutating_effect_expr(value),
        HirExpr::Binary { left, right, .. } => {
            first_mutating_effect_expr(left).or_else(|| first_mutating_effect_expr(right))
        }
        HirExpr::Field { base, .. } => first_mutating_effect_expr(base),
        HirExpr::Index { base, index, .. } => {
            first_mutating_effect_expr(base).or_else(|| first_mutating_effect_expr(index))
        }
        HirExpr::Call { args, .. } => args
            .iter()
            .find_map(|arg| first_mutating_effect_expr(&arg.value)),
        HirExpr::Closure { body, .. } => {
            body.statements.iter().find_map(first_mutating_effect_stmt)
        }
        HirExpr::Match { value, arms, .. } => first_mutating_effect_expr(value).or_else(|| {
            arms.iter().find_map(|arm| {
                arm.guard
                    .as_ref()
                    .and_then(first_mutating_effect_expr)
                    .or_else(|| first_mutating_effect_block(&arm.body))
            })
        }),
        HirExpr::MapLiteral { entries, .. } => entries.iter().find_map(|entry| {
            first_mutating_effect_expr(&entry.key)
                .or_else(|| first_mutating_effect_expr(&entry.value))
        }),
        HirExpr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| first_mutating_effect_expr(&field.value)),
        HirExpr::ArrayLiteral { items, .. } => items.iter().find_map(first_mutating_effect_expr),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn first_mutating_effect_block(
    block: &HirBlock,
) -> Option<(DataEffect, &crate::diagnostic::Span)> {
    block.statements.iter().find_map(first_mutating_effect_stmt)
}

pub(super) fn first_mutating_effect_stmt(
    stmt: &HirStmt,
) -> Option<(DataEffect, &crate::diagnostic::Span)> {
    match stmt {
        HirStmt::Let { value, .. } => value.as_ref().and_then(first_mutating_effect_expr),
        HirStmt::Return { value, .. } => value.as_ref().and_then(first_mutating_effect_expr),
        HirStmt::With { resource, body, .. } => {
            first_mutating_effect_expr(resource).or_else(|| first_mutating_effect_block(body))
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => first_mutating_effect_expr(condition)
            .or_else(|| first_mutating_effect_block(then_body))
            .or_else(|| else_body.as_ref().and_then(first_mutating_effect_block)),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(first_mutating_effect_expr)
            .or_else(|| first_mutating_effect_block(body)),
        HirStmt::For { iterable, body, .. } => {
            first_mutating_effect_expr(iterable).or_else(|| first_mutating_effect_block(body))
        }
        HirStmt::Match { value, arms, .. } => first_mutating_effect_expr(value).or_else(|| {
            arms.iter().find_map(|arm| {
                arm.guard
                    .as_ref()
                    .and_then(first_mutating_effect_expr)
                    .or_else(|| first_mutating_effect_block(&arm.body))
            })
        }),
        HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
            first_mutating_effect_expr(&arm.operation)
                .or_else(|| first_mutating_effect_block(&arm.body))
        }),
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => first_mutating_effect_expr(value),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => None,
    }
}

pub(super) fn check_expr_semantics_with_context(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    expr: &HirExpr,
    state: &BodyState,
    allow_weak_upgrade_arg: bool,
    async_call_consumed: bool,
    live_after: &HashSet<String>,
) {
    if let Some(diagnostic) = rsscript_semantics::integer_literal_range_diagnostic(expr) {
        analyzer.diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) = rsscript_semantics::char_literal_scalar_diagnostic(expr) {
        analyzer.diagnostics.push(diagnostic);
    }
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            span,
            ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution
                && let Some(diagnostic) = rsscript_semantics::async_call_consumption_diagnostic(
                    &body_callee_display(callee),
                    span,
                    signature.is_async,
                    async_call_consumed,
                )
            {
                analyzer.diagnostics.push(diagnostic);
            }
            check_constructor_field_initializers(analyzer, callee, args, expr, state);
            check_call_place_conflicts(analyzer, args, resolution, state);
            let weak_upgrade = rsscript_semantics::is_weak_upgrade_call(callee);
            let mut arg_live_after = live_after.clone();
            for arg in args.iter().rev() {
                if !tempdir_keep_consumes_resource_arg(callee, arg, state) {
                    check_expr_semantics_with_context(
                        analyzer,
                        local_analysis,
                        &arg.value,
                        state,
                        weak_upgrade,
                        false,
                        &arg_live_after,
                    );
                }
                collect_expr_uses(&arg.value, &mut arg_live_after);
            }
        }
        HirExpr::Spawn { value, .. } => {
            check_spawn_captures(analyzer, value, state);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                true,
                live_after,
            );
        }
        HirExpr::Await { value, .. } => {
            if let Some(diagnostic) = rsscript_semantics::await_operand_diagnostic(
                value,
                expr,
                &mut analyzer.async_let_names,
            ) {
                analyzer.diagnostics.push(diagnostic);
            }
            let mut await_live_after = live_after.clone();
            collect_await_operand_live_uses(value, &mut await_live_after);
            let mut await_live_facts = state
                .resources
                .iter()
                .map(|name| rsscript_semantics::AwaitLiveValueFact {
                    kind: "resource",
                    name: name.clone(),
                })
                .collect::<Vec<_>>();
            await_live_facts.extend(
                state
                    .locals
                    .iter()
                    .filter(|name| {
                        await_live_after.contains(*name)
                            && !state.value_type(name).is_some_and(is_copy_type_name)
                    })
                    .map(|name| rsscript_semantics::AwaitLiveValueFact {
                        kind: "local value",
                        name: name.clone(),
                    }),
            );
            analyzer
                .diagnostics
                .extend(rsscript_semantics::await_live_value_diagnostics(
                    hir_expr_span(expr),
                    &await_live_facts,
                ));
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                true,
                live_after,
            );
        }
        HirExpr::Effect {
            effect,
            value,
            span,
            ..
        } => {
            if matches!(effect, ParamEffect::Mut | ParamEffect::Take)
                && check_read_view_not_exclusive(analyzer, value, span, state)
            {
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    value,
                    state,
                    false,
                    async_call_consumed,
                    live_after,
                );
                return;
            }
            if matches!(effect, ParamEffect::Mut | ParamEffect::Take) && expr_is_fresh_shell(value)
            {
                fresh_requires_local_binding_diagnostic(analyzer, value, span);
            } else if *effect == ParamEffect::Take {
                check_take_operand_is_local(analyzer, value, span, state);
            } else if !(allow_weak_upgrade_arg && *effect == ParamEffect::Read)
                && matches!(effect, ParamEffect::Read | ParamEffect::Mut)
                && let Some(diagnostic) = rsscript_semantics::weak_field_upgrade_diagnostic(value)
            {
                analyzer.diagnostics.push(diagnostic);
            }
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                async_call_consumed,
                live_after,
            );
        }
        HirExpr::Try { value, .. } => {
            if let HirExpr::Try { span, .. } = expr
                && let Some(diagnostic) =
                    rsscript_semantics::try_operand_diagnostic(hir_expr_type_name(value), span)
            {
                analyzer.diagnostics.push(diagnostic);
            }
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                async_call_consumed,
                live_after,
            );
        }
        HirExpr::Manage { value, span, .. } => {
            check_manage_operand_is_local(analyzer, value, span, state);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Binary { left, right, .. } => {
            let mut left_live_after = live_after.clone();
            collect_expr_uses(right, &mut left_live_after);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                left,
                state,
                false,
                false,
                &left_live_after,
            );
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                right,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Field { base, .. } => {
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                base,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Index { base, index, .. } => {
            let mut base_live_after = live_after.clone();
            collect_expr_uses(index, &mut base_live_after);
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                base,
                state,
                false,
                false,
                &base_live_after,
            );
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                index,
                state,
                false,
                false,
                live_after,
            );
        }
        HirExpr::Closure { body, .. } => {
            let mut closure_state = BodyState::default();
            check_block(
                analyzer,
                local_analysis,
                body,
                &mut closure_state,
                false,
                &HashSet::new(),
            );
        }
        HirExpr::Match {
            value,
            scrutinee_effect,
            arms,
            type_name,
            ..
        } => {
            check_match_scrutinee_type(analyzer, value);
            check_match_patterns_match_scrutinee(analyzer, value, arms);
            check_match_pattern_effects(analyzer, value, *scrutinee_effect, arms);
            if *scrutinee_effect == Some(DataEffect::Take) {
                check_take_operand_is_local(analyzer, value, &arm_span(arms), state);
            }
            analyzer
                .diagnostics
                .extend(rsscript_semantics::match_expression_arm_type_diagnostics(
                    arms,
                    type_name.as_deref(),
                ));
            check_expr_semantics_with_context(
                analyzer,
                local_analysis,
                value,
                state,
                false,
                async_call_consumed,
                live_after,
            );
            let base_state = state.clone();
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    check_expr_semantics_with_context(
                        analyzer,
                        local_analysis,
                        guard,
                        state,
                        allow_weak_upgrade_arg,
                        async_call_consumed,
                        live_after,
                    );
                }
                let mut arm_state = base_state.clone();
                check_block(
                    analyzer,
                    local_analysis,
                    &arm.body,
                    &mut arm_state,
                    false,
                    live_after,
                );
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    &entry.key,
                    state,
                    allow_weak_upgrade_arg,
                    async_call_consumed,
                    live_after,
                );
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    &entry.value,
                    state,
                    allow_weak_upgrade_arg,
                    async_call_consumed,
                    live_after,
                );
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    &field.value,
                    state,
                    allow_weak_upgrade_arg,
                    async_call_consumed,
                    live_after,
                );
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                check_expr_semantics_with_context(
                    analyzer,
                    local_analysis,
                    item,
                    state,
                    allow_weak_upgrade_arg,
                    async_call_consumed,
                    live_after,
                );
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}
