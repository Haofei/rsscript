use super::*;

pub(super) fn check_manage_operand_is_local(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) {
    let Some(name) = hir_ident_name(value) else {
        // A freshly-produced, owned rvalue (a struct constructor or a
        // `fresh`-returning call) is sound to `manage` inline: the value has
        // just been created here and is not an alias of any existing managed,
        // borrowed, or local binding. This is the same freshness model that
        // lets a `fresh` value materialize directly. Every other rvalue
        // (idents, field/index projections, an existing `manage`, etc.) may
        // alias live state and is still rejected below.
        if expr_is_fresh_shell(value) {
            return;
        }
        let hoist = derive_manage_literal_hoist(analyzer.tokens, value, span, state);
        analyzer.diagnostics.push(
            rsscript_semantics::invalid_manage_operand_diagnostic_with_hoist(
                "`manage` can only move a named local binding or a freshly produced value.",
                span.clone(),
                hoist,
            ),
        );
        return;
    };
    if !state.is_local(name) {
        if state.is_read_view(name) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::read_view_mutation_diagnostic(
                    name,
                    span.clone(),
                ));
            return;
        }
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_manage_operand_diagnostic(
                format!("`{name}` is not a local binding and cannot be moved with `manage`."),
                span.clone(),
            ));
    }
}

pub(super) fn check_take_operand_is_local(
    analyzer: &mut Analyzer<'_>,
    value: &HirExpr,
    span: &Span,
    state: &BodyState,
) {
    let Some(path) = place_path(value) else {
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_take_operand_diagnostic(
                "`take` can only consume a named local binding or a local field path.",
                span.clone(),
            ));
        return;
    };
    if !state.is_local(&path.base) {
        if state.is_resource(&path.base) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::resource_escape_diagnostic(
                    &path.base,
                    span.clone(),
                ));
            return;
        }
        analyzer
            .diagnostics
            .push(rsscript_semantics::invalid_take_operand_diagnostic(
                format!(
                    "`{}` is not a local binding and cannot be consumed with `take`.",
                    path.base
                ),
                span.clone(),
            ));
    }
}

pub(super) fn tempdir_keep_consumes_resource_arg(
    callee: &Callee,
    arg: &HirCallArg,
    state: &BodyState,
) -> bool {
    let is_tempdir_keep = matches!(
        callee,
        Callee::Qualified { namespace, name } if namespace == "TempDir" && name == "keep"
    ) || matches!(callee, Callee::Name(name) if name == "TempDir.keep");
    if !is_tempdir_keep || arg.name.as_deref().unwrap_or("dir") != "dir" {
        return false;
    }
    let HirExpr::Effect {
        effect: ParamEffect::Take,
        value,
        ..
    } = &arg.value
    else {
        return false;
    };
    matches!(value.as_ref(), HirExpr::Ident { name, .. } if state.is_resource(name))
}

pub(super) fn hir_ident_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        _ => None,
    }
}

pub(super) fn apply_expr_effects(expr: &HirExpr, state: &mut BodyState) {
    match expr {
        HirExpr::Call { args, events, .. } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            for arg in args {
                apply_expr_effects(&arg.value, state);
            }
        }
        HirExpr::Effect { value, events, .. } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            apply_expr_effects(value, state);
        }
        HirExpr::Manage {
            value,
            events,
            span,
            ..
        } => {
            state.apply_retention_events(events);
            state.apply_move_events(events);
            if let Some(path) = place_path(value) {
                state.mark_moved(
                    &place_path_display(&path),
                    rsscript_semantics::MoveSite::manage(span.clone()),
                );
            }
            apply_expr_effects(value, state);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            apply_expr_effects(value, state)
        }
        HirExpr::Try { value, .. } => apply_expr_effects(value, state),
        HirExpr::Match {
            value,
            scrutinee_effect,
            span,
            ..
        } => {
            apply_expr_effects(value, state);
            apply_match_scrutinee_effect(*scrutinee_effect, value, span, state);
        }
        HirExpr::Binary { left, right, .. } => {
            apply_expr_effects(left, state);
            apply_expr_effects(right, state);
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                apply_expr_effects(&entry.key, state);
                apply_expr_effects(&entry.value, state);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                apply_expr_effects(&field.value, state);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                apply_expr_effects(item, state);
            }
        }
        HirExpr::Field { base, .. } => apply_expr_effects(base, state),
        HirExpr::Index { base, index, .. } => {
            apply_expr_effects(base, state);
            apply_expr_effects(index, state);
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn apply_match_scrutinee_effect(
    effect: Option<DataEffect>,
    value: &HirExpr,
    span: &Span,
    state: &mut BodyState,
) {
    if effect != Some(DataEffect::Take) {
        return;
    }
    if let Some(path) = place_path(value) {
        state.mark_moved(
            &place_path_display(&path),
            rsscript_semantics::MoveSite::take(span.clone()),
        );
    }
}

pub(super) fn arm_span(arms: &[HirMatchArm]) -> Span {
    arms.first().map_or_else(
        || Span {
            file: String::new(),
            line: 1,
            column: 1,
            length: 1,
        },
        |arm| arm.span.clone(),
    )
}

pub(super) fn check_moved_uses(analyzer: &mut Analyzer<'_>, local_analysis: &LocalAnalysis<'_>) {
    for moved_use in local_analysis.moved_uses() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::moved_use_diagnostic(
                &moved_use.name,
                moved_use.use_span,
                &moved_use.move_site,
            ));
    }
}

pub(super) fn check_managed_to_local_uses(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for managed_to_local in local_analysis.managed_to_local_uses() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::managed_to_local_diagnostic(
                &managed_to_local.local_name,
                &managed_to_local.managed_name,
                managed_to_local.span,
            ));
    }
}

pub(super) fn check_retained_local_uses(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for retained in local_analysis.retained_local_uses() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::retained_local_diagnostic(
                &retained.name,
                &retained.callee,
                &retained.param,
                retained.span,
            ));
    }
}

pub(super) fn check_retained_closure_captures(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for capture in local_analysis.retained_closure_captures() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::retained_closure_capture_diagnostic(
                &capture.name,
                &capture.callee,
                &capture.param,
                capture.capture_span,
                &capture.closure_span,
            ));
    }
}

pub(super) fn check_take_handle_fields(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
) {
    for field in local_analysis.take_handle_fields() {
        analyzer
            .diagnostics
            .push(rsscript_semantics::take_handle_field_diagnostic(
                &field.name,
                field.span.clone(),
            ));
    }
}

pub(super) fn check_fresh_returns(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis<'_>,
    function: &FunctionDecl,
) {
    if !function.returns_fresh {
        return;
    }
    // The `fresh Class`/`fresh Resource` rule reads only the signature, so it
    // belongs to the declaration pass, which sees interface declarations too
    // (`checks/declarations.rs::check_fresh_return_types`). What is left here
    // is the body rule: whether the value returned is actually clean.
    for issue in local_analysis.fresh_return_issues() {
        match &issue.kind {
            FreshReturnIssueKind::NotClean { name } => {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::fresh_return_not_clean_diagnostic(
                        &function.name,
                        name,
                        issue.span.clone(),
                    ));
            }
            FreshReturnIssueKind::UnknownIdent { name } if trusted_fresh_ident(analyzer, name) => {}
            FreshReturnIssueKind::UnknownIdent { .. } | FreshReturnIssueKind::Unknown => {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::freshness_unknown_diagnostic(
                        &function.name,
                        issue.span.clone(),
                    ));
            }
        }
    }
}

/// Derive the `manage <literal>` → `local <name> = <literal>` / `manage <name>`
/// repair for `RS0307`, or `None` when it cannot be derived exactly.
///
/// The card tells a model to manage an argument the callee retains, and the
/// shape models then write is `entry: manage "started"` — `manage` on a
/// literal, which is `RS0307`. The repair is mechanical, but it changes two
/// places at once: a new `local` line above the statement, and the operand
/// itself. A `Fix` carries one `FixEdit`, and one edit is one contiguous span,
/// so the edit produced here spans the whole region from the statement's first
/// column through the end of the literal and re-emits it.
///
/// Re-emitting means every character of that region has to be known, and the
/// only source this layer holds is the token stream. A token is reproduced
/// exactly when its rendered spelling fills its span; anything that does not —
/// an interpolated or multi-line string, an unknown character — ends the
/// derivation and the diagnostic advises instead. The other guards keep the
/// insertion point honest: the statement must start the line at paren/bracket
/// depth zero (so a multi-line argument list is left alone), nothing between
/// the statement start and the `manage` may open a nested body (`{`, `|`,
/// `=>`), and the hoisted name must be unused in the file and unambiguous —
/// the same literal managed twice would want the same name twice.
fn derive_manage_literal_hoist(
    tokens: &[Token],
    value: &HirExpr,
    manage_span: &Span,
    state: &BodyState,
) -> Option<ManageLiteralHoist> {
    let (literal_span, slug) = match value {
        HirExpr::String { value, span } => (span, identifier_slug(value)?),
        HirExpr::Number { value, span } => (span, identifier_slug(value)?),
        HirExpr::Char { value, span } => (span, identifier_slug(value)?),
        _ => return None,
    };
    if literal_span.line != manage_span.line || literal_span.file != manage_span.file {
        return None;
    }

    let manage_index = token_at(tokens, manage_span)?;
    let literal_index = token_at(tokens, literal_span)?;
    if literal_index != manage_index + 1 {
        return None;
    }
    let literal_text = token_source(&tokens[literal_index])?;

    let statement_index = statement_start_index(tokens, manage_index)?;
    let statement_span = &tokens[statement_index].span;
    if statement_span.line != manage_span.line {
        return None;
    }

    // The prefix is everything the statement already says before `manage`.
    let mut prefix = String::new();
    for token in &tokens[statement_index..manage_index] {
        if token.symbol("{") || token.symbol("|") || token.symbol("=>") || token.symbol("}") {
            return None;
        }
        let text = token_source(token)?;
        let offset = token.span.column.checked_sub(statement_span.column)?;
        while prefix.chars().count() < offset {
            prefix.push(' ');
        }
        if prefix.chars().count() != offset {
            return None;
        }
        prefix.push_str(&text);
    }
    // Keep whatever spacing separated the last prefix token from `manage`.
    let manage_offset = manage_span.column.checked_sub(statement_span.column)?;
    while prefix.chars().count() < manage_offset {
        prefix.push(' ');
    }
    if prefix.chars().count() != manage_offset {
        return None;
    }

    let name = format!("managed_{slug}");
    if state.locals.contains(&name)
        || state.managed.contains(&name)
        || state.value_types.contains_key(&name)
        || tokens.iter().any(|token| token.is_ident_text(&name))
    {
        return None;
    }
    // Two `manage` of the same literal would each want this one name.
    let occurrences = tokens
        .iter()
        .enumerate()
        .filter(|(index, token)| {
            token.is_ident_text("manage")
                && tokens
                    .get(index + 1)
                    .and_then(token_source)
                    .is_some_and(|text| text == literal_text)
        })
        .count();
    if occurrences != 1 {
        return None;
    }

    let indent = " ".repeat(statement_span.column.saturating_sub(1));
    let replacement = format!("local {name} = {literal_text}\n{indent}{prefix}manage {name}");
    let length = (literal_span.column + literal_span.length).checked_sub(statement_span.column)?;

    Some(ManageLiteralHoist {
        span: Span {
            file: statement_span.file.clone(),
            line: statement_span.line,
            column: statement_span.column,
            length,
        },
        replacement,
        name,
        literal: literal_text,
    })
}

/// The token whose span starts exactly at `span`, if it is still there.
fn token_at(tokens: &[Token], span: &Span) -> Option<usize> {
    tokens.iter().position(|token| {
        token.span.file == span.file
            && token.span.line == span.line
            && token.span.column == span.column
    })
}

/// The first token of the line `index` sits on, if a statement can start there.
///
/// A line that opens inside a `(` or `[` is a continuation of the statement
/// above it, so there is no place on it to insert a binding.
fn statement_start_index(tokens: &[Token], index: usize) -> Option<usize> {
    let line = tokens[index].span.line;
    let file = &tokens[index].span.file;
    let mut start = index;
    while start > 0 && tokens[start - 1].span.line == line && tokens[start - 1].span.file == *file {
        start -= 1;
    }
    let mut depth = 0i32;
    for token in &tokens[..start] {
        if token.symbol("(") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("]") {
            depth -= 1;
        }
    }
    (depth == 0).then_some(start)
}

/// The exact source spelling of `token`, or `None` when it cannot be known.
///
/// A token is reproduced only when its rendered spelling fills its recorded
/// span. That rules out escapes, interpolation and multi-line strings, whose
/// stored value is not what the source says.
fn token_source(token: &Token) -> Option<String> {
    let text = match &token.kind {
        TokenKind::Ident(value) | TokenKind::Number(value) => value.clone(),
        TokenKind::Keyword(value) | TokenKind::Symbol(value) => (*value).to_string(),
        TokenKind::String(value) => format!("\"{value}\""),
        TokenKind::Char(value) => format!("'{value}'"),
        TokenKind::InterpolatedString(_)
        | TokenKind::MultilineString(_)
        | TokenKind::Unknown(_)
        | TokenKind::Eof => return None,
    };
    (text.chars().count() == token.span.length).then_some(text)
}

/// An identifier-safe slug for a literal, or `None` when there is not one.
fn identifier_slug(value: &str) -> Option<String> {
    let mut slug = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            slug.extend(character.to_lowercase());
        } else if character == '_' || character == '-' || character == ' ' {
            if !slug.ends_with('_') && !slug.is_empty() {
                slug.push('_');
            }
        } else {
            return None;
        }
    }
    let slug = slug.trim_end_matches('_').to_string();
    (!slug.is_empty() && slug.len() <= 32).then_some(slug)
}
