use super::*;

impl Analyzer<'_> {
    pub(super) fn check_match_exhaustiveness(&mut self) {
        let function_names = self
            .syntax_program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(function) => Some(function.name.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        for function_name in function_names {
            let Some(body) = self
                .hir
                .function_body(&function_name)
                .and_then(|body| body.block.clone())
            else {
                continue;
            };
            self.check_match_exhaustiveness_block(&body);
        }
    }

    pub(super) fn check_match_exhaustiveness_block(&mut self, block: &HirBlock) {
        for statement in &block.statements {
            self.check_match_exhaustiveness_stmt(statement);
        }
    }

    pub(super) fn check_match_exhaustiveness_stmt(&mut self, statement: &HirStmt) {
        match statement {
            HirStmt::Let { value, .. } => {
                if let Some(value) = value {
                    self.check_match_exhaustiveness_expr(value);
                }
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.check_match_exhaustiveness_expr(value);
                }
            }
            HirStmt::With { resource, body, .. } => {
                self.check_match_exhaustiveness_expr(resource);
                self.check_match_exhaustiveness_block(body);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.check_match_exhaustiveness_expr(condition);
                self.check_match_exhaustiveness_block(then_body);
                if let Some(else_body) = else_body {
                    self.check_match_exhaustiveness_block(else_body);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.check_match_exhaustiveness_expr(condition);
                }
                self.check_match_exhaustiveness_block(body);
            }
            HirStmt::For { iterable, body, .. } => {
                self.check_match_exhaustiveness_expr(iterable);
                self.check_match_exhaustiveness_block(body);
            }
            HirStmt::Match {
                value, arms, span, ..
            } => {
                self.check_match_exhaustiveness_expr(value);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.check_match_exhaustiveness_expr(guard);
                    }
                    self.check_match_exhaustiveness_block(&arm.body);
                }
                if !self.match_is_exhaustive_with_context(value, arms) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NON_EXHAUSTIVE_MATCH,
                            "match statement is not exhaustive.",
                            span.clone(),
                            "non-exhaustive match",
                        )
                        .with_cause(
                            "Supported match statements must cover `Some`/`None`, `Ok`/`Err`, all sum type variants, or include `_`.",
                        )
                        .with_fix(
                            "add_missing_arm",
                            "Add the missing variant arm or a final `_` fallback.",
                            "manual",
                        ),
                    );
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    self.check_match_exhaustiveness_expr(&arm.operation);
                    self.check_match_exhaustiveness_block(&arm.body);
                }
            }
            HirStmt::Expr(expr) => self.check_match_exhaustiveness_expr(expr),
            HirStmt::Assign { target, value, .. } => {
                for read in crate::hir::assign_target_reads(target) {
                    self.check_match_exhaustiveness_expr(read);
                }
                self.check_match_exhaustiveness_expr(value);
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }

    pub(super) fn match_is_exhaustive_with_context(
        &self,
        value: &HirExpr,
        arms: &[HirMatchArm],
    ) -> bool {
        let Some(type_name) = hir_expr_type_name(value) else {
            let arm_names = arms
                .iter()
                .filter(|arm| arm.guard.is_none())
                .filter_map(|arm| arm.pattern.constructor_name().map(str::to_string))
                .collect();
            return builtin_match_is_exhaustive(&arm_names);
        };
        let patterns = arms
            .iter()
            .filter(|arm| arm.guard.is_none())
            .map(|arm| &arm.pattern)
            .collect::<Vec<_>>();
        self.patterns_cover_type(&patterns, type_name)
    }

    pub(super) fn patterns_cover_type(&self, patterns: &[&MatchPattern], type_name: &str) -> bool {
        if patterns.iter().any(|pattern| {
            matches!(
                pattern,
                MatchPattern::Wildcard(_) | MatchPattern::Binding { .. }
            )
        }) {
            return true;
        }
        let root = self.resolve_type_alias(type_root_name(type_name));
        if root == "List" {
            // A rest pattern `[a.., ..rest, ..z]` covers every length `>= a+z`; a
            // fixed pattern covers exactly its element count. The match is
            // exhaustive iff some rest pattern caps the open tail and every shorter
            // length is covered by a fixed pattern. With no rest pattern, infinitely
            // many lengths are uncovered, so an explicit `_` is required (handled
            // above).
            let mut min_rest: Option<usize> = None;
            let mut fixed_lengths = HashSet::new();
            for pattern in patterns {
                if let MatchPattern::List {
                    prefix,
                    rest,
                    suffix,
                    ..
                } = pattern
                {
                    let count = prefix.len() + suffix.len();
                    if rest.is_some() {
                        min_rest = Some(min_rest.map_or(count, |m: usize| m.min(count)));
                    } else {
                        fixed_lengths.insert(count);
                    }
                }
            }
            let Some(min_rest) = min_rest else {
                return false;
            };
            return (0..min_rest).all(|length| fixed_lengths.contains(&length));
        }
        if root == "Bool" {
            let bool_literals = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Literal {
                        value: crate::syntax::ast::MatchLiteral::Bool(value),
                        ..
                    } => Some(*value),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            return bool_literals.contains(&true) && bool_literals.contains(&false);
        }
        if root == "Option" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let some_has_irrefutable_payload = patterns.iter().any(|pattern| {
                matches!(pattern, MatchPattern::Variant { name, binding: None, .. } if name == "Some")
            });
            let some_patterns = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Variant { name, binding, .. } if name == "Some" => {
                        binding.as_deref()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let none_covered = patterns.iter().any(
                |pattern| matches!(pattern, MatchPattern::Variant { name, .. } if name == "None"),
            );
            let some_covered = some_has_irrefutable_payload
                || args
                    .first()
                    .is_some_and(|inner| self.patterns_cover_type(&some_patterns, inner));
            return some_covered && none_covered;
        }
        if root == "Result" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let ok_has_irrefutable_payload = patterns.iter().any(|pattern| {
                matches!(pattern, MatchPattern::Variant { name, binding: None, .. } if name == "Ok")
            });
            let err_has_irrefutable_payload = patterns.iter().any(|pattern| {
                matches!(pattern, MatchPattern::Variant { name, binding: None, .. } if name == "Err")
            });
            let ok_patterns = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Variant { name, binding, .. } if name == "Ok" => {
                        binding.as_deref()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let err_patterns = patterns
                .iter()
                .filter_map(|pattern| match pattern {
                    MatchPattern::Variant { name, binding, .. } if name == "Err" => {
                        binding.as_deref()
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let ok_covered = ok_has_irrefutable_payload
                || args
                    .first()
                    .is_some_and(|inner| self.patterns_cover_type(&ok_patterns, inner));
            let err_covered = err_has_irrefutable_payload
                || args
                    .get(1)
                    .is_some_and(|inner| self.patterns_cover_type(&err_patterns, inner));
            return ok_covered && err_covered;
        }
        if let Some(variants) = self.sum_variants_for_type(root) {
            return variants.iter().all(|(variant_name, fields)| {
                let matching = patterns
                    .iter()
                    .filter(|pattern| pattern.constructor_name() == Some(variant_name.as_str()))
                    .copied()
                    .collect::<Vec<_>>();
                self.patterns_cover_constructor_fields(&matching, fields)
            });
        }
        if matches!(
            self.hir.type_kind(root),
            Some(HirTypeKind::Struct | HirTypeKind::Class)
        ) {
            let fields = self
                .hir
                .type_info(root)
                .map(|info| info.fields.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            return self.patterns_cover_constructor_fields(patterns, &fields);
        }
        let arm_names = patterns
            .iter()
            .filter_map(|pattern| pattern.constructor_name().map(str::to_string))
            .collect();
        builtin_match_is_exhaustive(&arm_names)
    }

    pub(super) fn patterns_cover_constructor_fields(
        &self,
        patterns: &[&MatchPattern],
        fields: &[FieldInfo],
    ) -> bool {
        if fields.is_empty() {
            return !patterns.is_empty();
        }
        if patterns
            .iter()
            .any(|pattern| constructor_pattern_is_irrefutable(pattern))
        {
            return true;
        }
        let Some(witnesses) = self.field_witness_product(fields) else {
            return false;
        };
        witnesses.iter().all(|witness| {
            patterns
                .iter()
                .any(|pattern| self.pattern_matches_fields(pattern, witness))
        })
    }

    pub(super) fn field_witness_product(
        &self,
        fields: &[FieldInfo],
    ) -> Option<Vec<Vec<(String, PatternWitness)>>> {
        const MAX_PATTERN_WITNESSES: usize = 512;
        let mut rows: Vec<Vec<(String, PatternWitness)>> = vec![Vec::new()];
        for field in fields {
            let domain = self
                .finite_type_witnesses(&field.type_name)
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            if rows.len().saturating_mul(domain.len()) > MAX_PATTERN_WITNESSES {
                return None;
            }
            let mut next = Vec::new();
            for row in &rows {
                for witness in &domain {
                    let mut row = row.clone();
                    row.push((field.name.clone(), witness.clone()));
                    next.push(row);
                }
            }
            rows = next;
        }
        Some(rows)
    }

    pub(super) fn finite_type_witnesses(&self, type_name: &str) -> Option<Vec<PatternWitness>> {
        let root = self.resolve_type_alias(type_root_name(type_name));
        if root == "Bool" {
            return Some(vec![
                PatternWitness::Bool(true),
                PatternWitness::Bool(false),
            ]);
        }
        if root == "Option" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let payload = args
                .first()
                .and_then(|inner| self.finite_type_witnesses(inner))
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            let mut witnesses = vec![PatternWitness::Constructor {
                name: "None".to_string(),
                fields: Vec::new(),
            }];
            witnesses.extend(
                payload
                    .into_iter()
                    .map(|value| PatternWitness::Constructor {
                        name: "Some".to_string(),
                        fields: vec![("value".to_string(), value)],
                    }),
            );
            return Some(witnesses);
        }
        if root == "Result" {
            let args = type_arg_names(type_name).unwrap_or_default();
            let ok_payload = args
                .first()
                .and_then(|inner| self.finite_type_witnesses(inner))
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            let err_payload = args
                .get(1)
                .and_then(|inner| self.finite_type_witnesses(inner))
                .unwrap_or_else(|| vec![PatternWitness::Any]);
            let mut witnesses = Vec::new();
            witnesses.extend(
                ok_payload
                    .into_iter()
                    .map(|value| PatternWitness::Constructor {
                        name: "Ok".to_string(),
                        fields: vec![("value".to_string(), value)],
                    }),
            );
            witnesses.extend(
                err_payload
                    .into_iter()
                    .map(|value| PatternWitness::Constructor {
                        name: "Err".to_string(),
                        fields: vec![("value".to_string(), value)],
                    }),
            );
            return Some(witnesses);
        }
        if let Some(variants) = self.sum_variants_for_type(root) {
            let mut witnesses = Vec::new();
            for (variant_name, fields) in variants {
                let field_rows = self.field_witness_product(&fields)?;
                witnesses.extend(field_rows.into_iter().map(|fields| {
                    PatternWitness::Constructor {
                        name: variant_name.clone(),
                        fields,
                    }
                }));
            }
            return Some(witnesses);
        }
        if matches!(
            self.hir.type_kind(root),
            Some(HirTypeKind::Struct | HirTypeKind::Class)
        ) {
            let fields = self
                .hir
                .type_info(root)
                .map(|info| info.fields.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            return self.field_witness_product(&fields).map(|rows| {
                rows.into_iter()
                    .map(|fields| PatternWitness::Constructor {
                        name: root.to_string(),
                        fields,
                    })
                    .collect()
            });
        }
        None
    }

    pub(super) fn pattern_matches_fields(
        &self,
        pattern: &MatchPattern,
        fields: &[(String, PatternWitness)],
    ) -> bool {
        if constructor_pattern_is_irrefutable(pattern) {
            return true;
        }
        constrained_field_patterns(pattern)
            .into_iter()
            .all(|(name, pattern)| {
                fields
                    .iter()
                    .find(|(field_name, _)| field_name == &name)
                    .is_some_and(|(_, witness)| self.pattern_matches_witness(pattern, witness))
            })
    }

    pub(super) fn pattern_matches_witness(
        &self,
        pattern: &MatchPattern,
        witness: &PatternWitness,
    ) -> bool {
        match pattern {
            MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => true,
            MatchPattern::Literal {
                value: crate::syntax::ast::MatchLiteral::Bool(value),
                ..
            } => matches!(witness, PatternWitness::Bool(candidate) if candidate == value),
            MatchPattern::Literal { .. } | MatchPattern::List { .. } => false,
            MatchPattern::Variant { name, binding, .. } => {
                let PatternWitness::Constructor {
                    name: witness_name,
                    fields,
                } = witness
                else {
                    return false;
                };
                if name != witness_name {
                    return false;
                }
                if let Some(binding) = binding {
                    if matches!(
                        binding.as_ref(),
                        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                    ) {
                        return true;
                    }
                    fields
                        .first()
                        .is_some_and(|(_, witness)| self.pattern_matches_witness(binding, witness))
                } else {
                    true
                }
            }
            MatchPattern::Struct {
                name, fields: _, ..
            } => {
                let PatternWitness::Constructor {
                    name: witness_name,
                    fields,
                } = witness
                else {
                    return false;
                };
                name == witness_name && self.pattern_matches_fields(pattern, fields)
            }
        }
    }

    /// A payload-less sum type (every variant carries no fields) behaves like a
    /// plain C-style enum: it is cheap to copy and may be passed by value without
    /// an explicit data effect, mirroring how primitive Copy types are treated.
    pub(super) fn is_payloadless_sum_type(&self, ty: &TypeRef) -> bool {
        if !ty.args.is_empty() || ty.is_noescape || ty.is_owned || ty.is_fresh {
            return false;
        }
        let root = type_root_name(&ty.name);
        if self.hir.type_kind(root) != Some(HirTypeKind::Sum) {
            return false;
        }
        match self.sum_variants_for_type(root) {
            Some(variants) => variants.iter().all(|(_, fields)| fields.is_empty()),
            None => false,
        }
    }

    pub(super) fn sum_variants_for_type(
        &self,
        root: &str,
    ) -> Option<Vec<(String, Vec<FieldInfo>)>> {
        for item in &self.syntax_program.items {
            if let Item::SumType(sum) = item
                && sum.name == root
            {
                let variants = sum
                    .variants
                    .iter()
                    .map(|variant| {
                        (
                            variant.name.clone(),
                            self.hir
                                .sum_variant_fields(&variant.name)
                                .map(<[FieldInfo]>::to_vec)
                                .unwrap_or_default(),
                        )
                    })
                    .collect();
                return Some(variants);
            }
        }
        None
    }

    pub(super) fn check_match_exhaustiveness_expr(&mut self, expr: &HirExpr) {
        match expr {
            HirExpr::Binary { left, right, .. } => {
                self.check_match_exhaustiveness_expr(left);
                self.check_match_exhaustiveness_expr(right);
            }
            HirExpr::Field { base, .. } => self.check_match_exhaustiveness_expr(base),
            HirExpr::Index { base, index, .. } => {
                self.check_match_exhaustiveness_expr(base);
                self.check_match_exhaustiveness_expr(index);
            }
            HirExpr::Call { args, .. } => {
                for arg in args {
                    self.check_match_exhaustiveness_expr(&arg.value);
                }
            }
            HirExpr::Effect { value, .. }
            | HirExpr::Manage { value, .. }
            | HirExpr::Spawn { value, .. }
            | HirExpr::Await { value, .. }
            | HirExpr::Try { value, .. } => {
                self.check_match_exhaustiveness_expr(value);
            }
            HirExpr::Closure { body, .. } => self.check_match_exhaustiveness_block(body),
            HirExpr::Match {
                value, arms, span, ..
            } => {
                self.check_match_exhaustiveness_expr(value);
                for arm in arms {
                    self.check_match_exhaustiveness_block(&arm.body);
                }
                if !self.match_is_exhaustive_with_context(value, arms) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NON_EXHAUSTIVE_MATCH,
                            "match expression is not exhaustive.",
                            span.clone(),
                            "non-exhaustive match",
                        )
                        .with_cause(
                            "Supported match expressions must cover `Some`/`None`, `Ok`/`Err`, all sum type variants, or include `_`.",
                        )
                        .with_fix(
                            "add_missing_arm",
                            "Add the missing variant arm or a final `_` fallback.",
                            "manual",
                        ),
                    );
                }
            }
            HirExpr::ObjectLiteral { .. }
            | HirExpr::MapLiteral { .. }
            | HirExpr::ArrayLiteral { .. }
            | HirExpr::Ident { .. }
            | HirExpr::Number { .. }
            | HirExpr::String { .. }
            | HirExpr::Unknown(_) => {}
        }
    }
}
