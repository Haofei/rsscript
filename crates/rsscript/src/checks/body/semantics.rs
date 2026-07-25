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
                    check_resource_pool_lease_expr(analyzer, value, false);
                    check_resource_producer_expr(analyzer, value, false);
                }
            }

            Flow::Fallthrough
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                check_expr_semantics(analyzer, local_analysis, value, state, live_after);
                if check_resource_contexts {
                    check_resource_pool_lease_expr(analyzer, value, false);
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
                check_resource_pool_lease_expr(analyzer, resource, true);
                check_result_resource_with_has_try(analyzer, resource);
                check_resource_producer_expr(analyzer, resource, true);
                check_resource_escape(analyzer, local_analysis, span);
            }
            if check_resource_contexts
                && let Some(pool_path) = resource_pool_borrow_pool_path(resource)
            {
                check_resource_pool_active_lease_block(analyzer, &pool_path, body);
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
            check_bool_condition(analyzer, condition, "if");
            check_expr_semantics(analyzer, local_analysis, condition, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, condition, false);
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
                check_bool_condition(analyzer, condition, "while");
                check_expr_semantics(analyzer, local_analysis, condition, state, live_after);
                if check_resource_contexts {
                    check_resource_pool_lease_expr(analyzer, condition, false);
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
            check_for_iterable_type(analyzer, iterable, iterable_type_name.as_deref(), *is_async);
            check_expr_semantics(analyzer, local_analysis, iterable, state, live_after);
            if check_resource_contexts {
                check_resource_pool_lease_expr(analyzer, iterable, false);
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
                check_resource_pool_lease_expr(analyzer, value, false);
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
                    check_resource_pool_lease_expr(analyzer, &arm.operation, false);
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
                check_resource_pool_lease_expr(analyzer, expr, false);
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
            type_name,
            ..
        } => {
            match kind {
                HirBindingKind::ManagedLet => state.bind_managed(name.clone()),
                HirBindingKind::LocalLet => state.bind_local(name.clone()),
                HirBindingKind::Param => {}
            }
            if let Some(type_name) = type_name {
                state.record_type(name.clone(), type_name.clone());
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

/// A decimal integer literal must fit RSScript's `Int` (i64). Reject out-of-range
/// literals at the frontend so they never reach the VM (runtime error) or the
/// compiled backend (rustc error). Non-decimal / float literals are left alone.
pub(super) fn check_integer_literal_range(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    let HirExpr::Number { value, span, .. } = expr else {
        return;
    };
    let is_decimal_integer = !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    if is_decimal_integer && value.parse::<i64>().is_err() {
        analyzer.diagnostics.push(error_cause_manual_fix(
            code::INTEGER_LITERAL_OUT_OF_RANGE,
            format!("integer literal `{value}` does not fit in `Int` (i64)."),
            span.clone(),
            "integer literal out of range",
            "RSScript `Int` is a 64-bit signed integer; literals must fit in i64.",
            "use_in_range_literal",
            "Use a value within i64 range.",
        ));
    }
}

/// A `Char` literal denotes exactly one Unicode scalar. Reject `''` (empty) and
/// `'ab'` (multi-scalar) at the frontend so they never reach lowering (which
/// would otherwise silently truncate to the first scalar) or the VM/compiled
/// backend. Well-formed single-scalar literals are left alone.
pub(super) fn check_char_literal_scalar(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    let HirExpr::Char { value, span } = expr else {
        return;
    };
    let count = crate::text_util::char_literal_scalar_count(value);
    if count != 1 {
        analyzer.diagnostics.push(error_cause_manual_fix(
            code::CHAR_LITERAL_NOT_SINGLE_SCALAR,
            format!("character literal must contain exactly one character, found {count}."),
            span.clone(),
            "invalid character literal",
            "A `Char` is a single Unicode scalar value; `''` is empty and `'ab'` holds more than one.",
            "use_single_char_literal",
            "Put exactly one character between the single quotes, or use a `String` literal (double quotes) for text.",
        ));
    }
}

pub(super) fn check_bool_condition(analyzer: &mut Analyzer<'_>, expr: &HirExpr, construct: &str) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    if type_name == "Bool" {
        return;
    }
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("{construct} condition has type `{type_name}`, expected `Bool`."),
        hir_expr_span(expr).clone(),
        "control-flow type mismatch",
        "RSScript control-flow conditions are explicit `Bool` values; non-empty strings, numbers, and managed handles do not coerce to truthy or falsey values.",
        "use_bool_condition",
        "Compare explicitly or call a function that returns `Bool`.",
    ));
}

pub(super) fn check_for_iterable_type(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    type_name: Option<&str>,
    is_async: bool,
) {
    let Some(type_name) = type_name else {
        return;
    };
    let bare_type_name = strip_fresh_type(type_name);
    if (!is_async && list_element_type(bare_type_name).is_some())
        || (is_async && stream_item_type(bare_type_name).is_some())
    {
        return;
    }
    let expected = if is_async { "Stream<T>" } else { "List<T>" };
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("for iterable has type `{type_name}`, expected `{expected}`."),
        hir_expr_span(expr).clone(),
        "control-flow type mismatch",
        if is_async {
            "RSScript `await for` iterates `Stream<T>` values by repeatedly awaiting `Stream.next`."
        } else {
            "RSScript v0.7 `for` iteration is limited to `List<T>` so loop ownership and review metadata stay explicit."
        },
        if is_async { "iterate_stream" } else { "iterate_list" },
        if is_async {
            "Iterate a `Stream<T>` value or convert the input to a Stream before the loop."
        } else {
            "Iterate a `List<T>` value or convert the input to a List before the loop."
        },
    ));
}

pub(super) fn check_match_scrutinee_type(analyzer: &mut Analyzer<'_>, expr: &HirExpr) {
    let Some(type_name) = hir_expr_type_name(expr) else {
        return;
    };
    let type_name = analyzer.expand_type_alias(type_name);
    if matches!(type_root_name(&type_name), "Option" | "Result" | "List") {
        return;
    }
    if matches!(type_name.as_str(), "Int" | "String" | "Char" | "Bool") {
        return;
    }
    // Allow matching on user-defined sum/struct/class types.
    if matches!(
        analyzer.hir.type_kind(&type_name),
        Some(HirTypeKind::Sum | HirTypeKind::Struct | HirTypeKind::Class)
    ) {
        return;
    }
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("match scrutinee has type `{type_name}`, expected `Option<T>`, `Result<T, E>`, `List<T>`, a declared sum/struct/class type, or an `Int`/`String`/`Char`/`Bool` literal match."),
        hir_expr_span(expr).clone(),
        "control-flow type mismatch",
        "RSScript v0.7 `match` is limited to review-visible `Option`, `Result`, declared sum/struct/class patterns, and simple scalar literal dispatch.",
        "match_option_or_result",
        "Match an `Option<T>`, `Result<T, E>`, declared sum value, or scalar literal value; otherwise rewrite this branch as `if`.",
    ));
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
            if let MatchLiteral::Char(raw) = value {
                if crate::text_util::char_literal_scalar_count(raw) != 1 {
                    analyzer.diagnostics.push(error_cause_manual_fix(
                        code::CHAR_LITERAL_NOT_SINGLE_SCALAR,
                        format!(
                            "character literal must contain exactly one character, found {}.",
                            crate::text_util::char_literal_scalar_count(raw)
                        ),
                        span.clone(),
                        "invalid character literal",
                        "A `Char` is a single Unicode scalar value; `''` is empty and `'ab'` holds more than one.",
                        "use_single_char_literal",
                        "Put exactly one character between the single quotes.",
                    ));
                }
            }
            let literal_type = match value {
                MatchLiteral::Int(_) => "Int",
                MatchLiteral::String(_) => "String",
                MatchLiteral::Char(_) => "Char",
                MatchLiteral::Bool(_) => "Bool",
            };
            if literal_type != type_name {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::CONTROL_FLOW_TYPE_MISMATCH,
                        format!("literal match pattern cannot match scrutinee type `{type_name}`."),
                        span.clone(),
                        "match literal type mismatch",
                    )
                    .with_cause(
                        "Literal patterns are only allowed when matching `Int`, `String`, or `Bool` values.",
                    ),
                );
            }
        }
        MatchPattern::Variant {
            name,
            bindings,
            span,
            ..
        } if root == "Option" => {
            if !matches!(name.as_str(), "Some" | "None") {
                push_match_variant_type_mismatch(
                    analyzer,
                    name,
                    &type_name,
                    &["Some".to_string(), "None".to_string()],
                    span,
                );
                return;
            }
            // `Some` carries one payload, `None` carries none.
            let expected = if name == "Some" { 1 } else { 0 };
            if !bindings.is_empty() && bindings.len() != expected {
                push_variant_pattern_arity_mismatch(analyzer, name, expected, bindings.len(), span);
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
                push_match_variant_type_mismatch(
                    analyzer,
                    name,
                    &type_name,
                    &["Ok".to_string(), "Err".to_string()],
                    span,
                );
                return;
            }
            if !bindings.is_empty() && bindings.len() != 1 {
                push_variant_pattern_arity_mismatch(analyzer, name, 1, bindings.len(), span);
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
                    push_variant_or_struct_cannot_match(analyzer, name, &type_name, span);
                } else {
                    push_match_variant_type_mismatch(analyzer, name, &type_name, &allowed, span);
                }
                return;
            };
            // A bare variant name (`V`) matches without binding any field. Once a
            // positional payload is written, its arity must equal the declared
            // field count, and each sub-pattern is checked against the field type
            // at the same position (the RS0037 safety net for positional binding).
            if !bindings.is_empty() && bindings.len() != fields.len() {
                push_variant_pattern_arity_mismatch(
                    analyzer,
                    name,
                    fields.len(),
                    bindings.len(),
                    span,
                );
                return;
            }
            for (binding, field) in bindings.iter().zip(fields.iter()) {
                check_match_pattern_matches_type(analyzer, binding, &field.type_name);
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
                push_variant_or_struct_cannot_match(analyzer, name, &type_name, span);
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
                    let field_type = substitute_type_args(&field_info.type_name, &substitutions);
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
                push_variant_or_struct_cannot_match(analyzer, "[..]", &type_name, span);
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

pub(super) fn push_variant_or_struct_cannot_match(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    type_name: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("match pattern `{name}` cannot match scrutinee type `{type_name}`."),
            span.clone(),
            "match pattern type mismatch",
        )
        .with_cause("RSScript match patterns must belong to the scrutinee's type."),
    );
}

pub(super) fn push_variant_pattern_arity_mismatch(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    expected: usize,
    found: usize,
    span: &Span,
) {
    let field_word = if expected == 1 { "field" } else { "fields" };
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::VARIANT_PATTERN_ARITY_MISMATCH,
            format!(
                "variant pattern `{name}` binds {found} sub-pattern(s) but `{name}` declares {expected} {field_word}."
            ),
            span.clone(),
            "variant pattern arity mismatch",
        )
        .with_cause(
            "A positional variant pattern must bind exactly as many sub-patterns as the variant declares fields, in declared order.",
        ),
    );
}

pub(super) fn push_match_variant_type_mismatch(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    type_name: &str,
    allowed: &[String],
    span: &Span,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("match pattern `{name}` cannot match scrutinee type `{type_name}`."),
        span.clone(),
        "match variant type mismatch",
        "RSScript match patterns must belong to the scrutinee's type.",
        "match_matching_variant_family",
        format!(
            "Use variants of `{}`: {}.",
            type_root_name(type_name),
            allowed.join(", ")
        ),
    ));
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
    if scrutinee_effect == Some(DataEffect::Take)
        && !analyzer.syntax_program.has_feature(FileFeature::Local)
        && let Some(first_arm) = arms.first()
    {
        analyzer.diagnostics.push(error_cause_manual_fix(
            code::FEATURE_VIOLATION,
            "`match take` requires `features: local`.",
            first_arm.span.clone(),
            "missing local feature",
            "Taking fields out of a pattern is a local ownership capability.",
            "add_local_feature",
            "Add `features: local` or match the value with `read`.",
        ));
    }
    for arm in arms {
        if matches!(
            arm.pattern,
            MatchPattern::Struct { .. } | MatchPattern::List { .. }
        ) && scrutinee_effect.is_none()
        {
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::MISSING_DATA_EFFECT,
                "structured match patterns require an explicit scrutinee effect.",
                arm.span.clone(),
                "missing match scrutinee effect",
                "A structured pattern projects fields from the scrutinee, so the source must spell `match read`, `match mut`, or `match take`.",
                "spell_match_effect",
                "Add `read`, `mut`, or `take` after `match`.",
            ));
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
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::READ_VIEW_MUTATION,
                format!("match guard cannot use `{}`.", effect.as_str()),
                span.clone(),
                "guard mutation is not allowed",
                "A guard runs before the arm is selected and may only read pattern bindings.",
                "make_guard_read_only",
                "Move mutation into the selected arm body or rewrite the guard as a read-only predicate.",
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
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::READ_VIEW_MUTATION,
                format!(
                    "managed pattern field `{}` cannot request `{}`.",
                    field.name,
                    effect.as_str()
                ),
                field.span.clone(),
                "managed pattern field is read-only",
                "Managed class values are shared runtime objects; structured patterns expose only read views of their fields.",
                "use_read_pattern",
                "Use a read field binding and perform managed mutation through an explicit method.",
            ));
            continue;
        }
        let allowed = match scrutinee_effect {
            Some(DataEffect::Read) => effect == DataEffect::Read,
            Some(DataEffect::Mut) => matches!(effect, DataEffect::Read | DataEffect::Mut),
            Some(DataEffect::Take) => true,
            None => effect == DataEffect::Read,
        };
        if !allowed {
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::READ_VIEW_MUTATION,
                format!(
                    "field pattern `{}` requests `{}` from a weaker match scrutinee.",
                    field.name,
                    effect.as_str()
                ),
                field.span.clone(),
                "pattern effect is not allowed",
                "Pattern binding effects are monotonic: a child field cannot request more authority than the scrutinee effect provides.",
                "weaken_pattern_effect",
                "Use `read` for this field or strengthen the match scrutinee effect when the value is local and mutable.",
            ));
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
    check_integer_literal_range(analyzer, expr);
    check_char_literal_scalar(analyzer, expr);
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            span,
            ..
        } => {
            check_async_call_consumed(analyzer, callee, resolution, span, async_call_consumed);
            check_constructor_field_initializers(analyzer, callee, args, expr, state);
            check_call_place_conflicts(analyzer, args, resolution, state);
            check_resource_pool_new_factory_contract(analyzer, callee, args);
            check_resource_pool_try_new_factory_contract(analyzer, callee, args);
            check_resource_pool_constructor_max_size_contract(analyzer, callee, args);
            check_resource_pool_factory_resource_captures(analyzer, callee, args, state);
            let weak_upgrade = is_weak_upgrade_callee(callee);
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
            check_await_operand(analyzer, value, expr);
            let mut await_live_after = live_after.clone();
            collect_await_operand_live_uses(value, &mut await_live_after);
            check_await_live_values(analyzer, state, expr, &await_live_after);
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
            {
                check_weak_field_requires_upgrade(analyzer, value);
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
            if let HirExpr::Try { span, .. } = expr {
                check_try_value_is_result(analyzer, value, span);
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
            check_match_expression_arm_types(analyzer, arms, type_name.as_deref());
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

pub(super) fn check_match_expression_arm_types(
    analyzer: &mut Analyzer<'_>,
    arms: &[HirMatchArm],
    expected_type: Option<&str>,
) {
    let Some(expected_type) = expected_type else {
        return;
    };
    for arm in arms {
        let Some(arm_type) = match_arm_value_type(&arm.body) else {
            continue;
        };
        if arm_type == expected_type {
            continue;
        }
        analyzer.diagnostics.push(error_cause_manual_fix(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!(
                "match arm has type `{arm_type}`, expected `{expected_type}` from the first produced arm."
            ),
            arm.span.clone(),
            "match arm type mismatch",
            "A match expression must produce one compatible value type across every arm.",
            "align_match_arm_types",
            "Return the same value type from every match expression arm.",
        ));
    }
}

pub(super) fn match_arm_value_type(block: &HirBlock) -> Option<&str> {
    match block.statements.iter().next_back()? {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => hir_expr_type_name(value),
        _ => None,
    }
}
