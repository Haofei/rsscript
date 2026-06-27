use crate::syntax::ast::{DataEffect, Expr, FieldDecl, Item, MatchPattern, TypeRef};

use super::helpers::*;

use super::lowerer::*;

impl RustLowerer<'_> {
    pub(super) fn lower_json_value(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => self.lower_json_value(value),
            Expr::ObjectLiteral { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| self.lower_json_field(&field.name, &field.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("rsscript_runtime::json_object(&vec![{fields}])")
            }
            Expr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_json_array_item(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("rsscript_runtime::json_array(&vec![{items}])")
            }
            _ => self.lower_json_array_item(expr),
        }
    }

    pub(super) fn lower_map_literal(&mut self, expr: &Expr, expected: &TypeRef) -> String {
        let Expr::MapLiteral { entries, .. } = expr else {
            return self.lower_expr(expr);
        };
        let key_type = expected.args.first();
        let value_type = expected.args.get(1);
        let entries = entries
            .iter()
            .map(|entry| {
                let key = if let Some(expected) = key_type {
                    self.lower_retained_expr_for_expected_type(&entry.key, expected)
                } else {
                    self.lower_owned_expr(&entry.key)
                };
                let value = if let Some(expected) = value_type {
                    self.lower_retained_expr_for_expected_type(&entry.value, expected)
                } else {
                    self.lower_owned_expr(&entry.value)
                };
                format!("({key}, {value})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("rsscript_runtime::map_from_entries(vec![{entries}])")
    }

    pub(super) fn lower_list_literal(&mut self, expr: &Expr, expected: &TypeRef) -> String {
        let Expr::ArrayLiteral { items, .. } = expr else {
            return self.lower_expr(expr);
        };
        let item_type = expected.args.first();
        let items = items
            .iter()
            .map(|item| {
                if let Some(expected) = item_type {
                    self.lower_retained_expr_for_expected_type(item, expected)
                } else {
                    self.lower_owned_expr(item)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("vec![{items}]")
    }

    pub(super) fn lower_json_field(&mut self, name: &str, value: &Expr) -> String {
        let key = format!("{:?}.to_string()", decode_string_token(name));
        match value {
            Expr::ObjectLiteral { .. } | Expr::ArrayLiteral { .. } => {
                let lowered = self.lower_json_value(value);
                format!("rsscript_runtime::json_raw_field(&{key}, &{lowered})")
            }
            Expr::Ident(name, _) if name == "true" || name == "false" => {
                format!("rsscript_runtime::json_bool_field(&{key}, {name})")
            }
            Expr::Ident(name, _) if name == "null" => {
                format!("rsscript_runtime::json_raw_field(&{key}, \"null\")")
            }
            _ => match self.infer_expr_type(value).map(|ty| ty.name) {
                Some(ty) if ty == "Int" => {
                    format!(
                        "rsscript_runtime::json_int_field(&{key}, {})",
                        self.lower_expr(value)
                    )
                }
                Some(ty) if ty == "Bool" => format!(
                    "rsscript_runtime::json_bool_field(&{key}, {})",
                    self.lower_expr(value)
                ),
                Some(ty) if ty == "JsonLiteral" => {
                    let lowered = self.lower_expr(value);
                    format!("rsscript_runtime::json_raw_field(&{key}, &{lowered})")
                }
                Some(ty) if ty == "JsonValue" => {
                    let lowered = self.lower_expr(value);
                    format!(
                        "rsscript_runtime::json_raw_field(&{key}, &rsscript_runtime::json_to_string(&{lowered}))"
                    )
                }
                _ => format!(
                    "rsscript_runtime::json_string_field(&{key}, {})",
                    self.lower_json_string_value(value)
                ),
            },
        }
    }

    pub(super) fn lower_json_array_item(&mut self, value: &Expr) -> String {
        match value {
            Expr::ObjectLiteral { .. } | Expr::ArrayLiteral { .. } => self.lower_json_value(value),
            Expr::Ident(name, _) if name == "true" || name == "false" => name.clone(),
            Expr::Ident(name, _) if name == "null" => "null".to_string(),
            _ => match self.infer_expr_type(value).map(|ty| ty.name) {
                Some(ty) if ty == "Int" || ty == "Bool" => self.lower_expr(value),
                Some(ty) if ty == "JsonLiteral" => self.lower_expr(value),
                Some(ty) if ty == "JsonValue" => {
                    format!(
                        "rsscript_runtime::json_to_string(&{})",
                        self.lower_expr(value)
                    )
                }
                _ => format!(
                    "rsscript_runtime::json_quote_string({})",
                    self.lower_json_string_value(value)
                ),
            },
        }
    }

    pub(super) fn lower_json_string_value(&mut self, value: &Expr) -> String {
        match value {
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                ..
            } => self.lower_json_string_value(value),
            Expr::Ident(name, _) if self.param_effects.contains_key(name) => self.lower_expr(value),
            _ => format!("&{}", self.lower_expr(value)),
        }
    }

    pub(super) fn lower_match_scrutinee_expr(
        &mut self,
        value: &Expr,
        scrutinee_type: Option<&TypeRef>,
    ) -> String {
        let lowered = self.lower_expr(value);
        if scrutinee_type.is_some_and(|ty| ty.name == "String") {
            format!("{lowered}.as_str()")
        } else if self.match_scrutinee_is_forced_borrow(value) {
            format!("&({lowered})")
        } else {
            lowered
        }
    }

    // A match scrutinee that is a *place* behind a borrow (a field/index of a read-view) has
    // value type `T` (not `&T`), so matching it would move a non-Copy payload out of a shared
    // reference. Borrow it so the match binds by reference instead.
    pub(super) fn match_scrutinee_is_forced_borrow(&self, value: &Expr) -> bool {
        matches!(value, Expr::Field { .. } | Expr::Index { .. })
            && self.match_scrutinee_by_ref(value)
    }

    // Whether a match scrutinee lowers to a reference (`&T`): a read-view `let` binding, a `read`
    // param, or a field/index of one. Such matches bind payloads by reference (so non-Copy payloads
    // don't move out of a shared ref, and Copy payloads need a deref-pattern to come out by value).
    pub(super) fn match_scrutinee_by_ref(&self, value: &Expr) -> bool {
        match value {
            Expr::Ident(name, _) => {
                self.read_view_bindings.contains(name)
                    || self.param_effects.get(name) == Some(&DataEffect::Read)
            }
            Expr::Field { base, .. } | Expr::Index { base, .. } => {
                self.match_scrutinee_by_ref(base)
            }
            Expr::Effect { value, .. } | Expr::Try { value, .. } => {
                self.match_scrutinee_by_ref(value)
            }
            _ => false,
        }
    }

    // For a match arm that binds a *single* payload field to a name, return
    // `(binding_name, payload_field_type)`. Covers the built-in `Option<T>` /
    // `Result<T, E>` variants and single-field user sum-type variants; `None`
    // otherwise (nullary variant, wildcard payload, multi-field, etc.).
    pub(super) fn single_payload_binding(
        &self,
        pattern: &MatchPattern,
        value_type: Option<&TypeRef>,
    ) -> Option<(String, TypeRef)> {
        let MatchPattern::Variant { name, binding, .. } = pattern else {
            return None;
        };
        let MatchPattern::Binding {
            name: bind_name, ..
        } = binding.as_deref()?
        else {
            return None;
        };
        let field_ty = match (name.as_str(), value_type) {
            ("Some", Some(ty)) if ty.name == "Option" => ty.args.first().cloned()?,
            ("Ok", Some(ty)) if ty.name == "Result" => ty.args.first().cloned()?,
            ("Err", Some(ty)) if ty.name == "Result" => ty.args.get(1).cloned()?,
            _ => {
                let (_, fields) = self.sum_variant_fields_for_type(value_type, name)?;
                if fields.len() != 1 {
                    return None;
                }
                fields[0].ty.clone()
            }
        };
        Some((bind_name.clone(), field_ty))
    }

    // When the scrutinee is matched by-ref, a single payload binding is `&T` (match
    // ergonomics), but RSScript's model is that the arm sees an owned `T` — so using
    // it by value (`return s`, passing it to a by-value param) must work without the
    // user knowing the Rust representation. Return `(name, owned_rhs)` pairs so the
    // arm can shadow the borrowed binding: `*x` for a `Copy` payload, `x.clone()`
    // for any other cloneable value type. Resources aren't `Clone` and can't be
    // moved out of a shared `read` view, so they are left as `&T` (the resource
    // move rules reject using them by value).
    pub(super) fn owned_payload_rebindings(
        &self,
        pattern: &MatchPattern,
        value_type: Option<&TypeRef>,
    ) -> Vec<(String, String)> {
        // Single-payload variants (`Some(x)`, `Ok(x)`, single-field user variant).
        if let Some((bind_name, field_ty)) = self.single_payload_binding(pattern, value_type) {
            return self
                .owned_rebinding_for(&bind_name, &field_ty)
                .into_iter()
                .collect();
        }
        // Struct / tuple patterns: each bound field of a by-ref scrutinee is `&T`
        // under match ergonomics, but the arm should see an owned `T` — so shadow
        // every binding with its owned form, recursing through nested patterns.
        if let MatchPattern::Struct { name, fields, .. } = pattern {
            let declared = self.pattern_declared_field_types(value_type, name);
            let params = self.pattern_type_params(value_type, name);
            let args: Vec<TypeRef> = value_type.map(|ty| ty.args.clone()).unwrap_or_default();
            let mut rebindings = Vec::new();
            for field in fields {
                if field.ignored {
                    continue;
                }
                let declared_ty = declared.as_ref().and_then(|fields| {
                    fields
                        .iter()
                        .find(|candidate| candidate.name == field.name)
                        .map(|field| substitute_generic_type(&field.ty, &params, &args))
                });
                if let Some(binding) = &field.binding {
                    // `mut` bindings stay borrowed: rebinding would discard the
                    // mutable view the user asked for.
                    if field.effect == Some(crate::syntax::ast::DataEffect::Mut) {
                        continue;
                    }
                    if let Some(field_ty) = &declared_ty {
                        rebindings.extend(self.owned_rebinding_for(binding, field_ty));
                    }
                } else if let Some(nested) = &field.pattern {
                    rebindings.extend(self.owned_payload_rebindings(nested, declared_ty.as_ref()));
                }
            }
            return rebindings;
        }
        // List slice patterns always bind through a `&[T]` slice (the scrutinee is
        // matched via `.as_slice()`), so element bindings are `&T` and the rest
        // binding is `&[T]`; rebind each to its owned `T` / `List<T>` form.
        if let MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } = pattern
        {
            let element_ty = value_type.and_then(|ty| ty.args.first());
            let mut rebindings = Vec::new();
            for element in prefix.iter().chain(suffix) {
                if let MatchPattern::Binding { name, .. } = element {
                    if let Some(element_ty) = element_ty {
                        rebindings.extend(self.owned_rebinding_for(name, element_ty));
                    }
                } else {
                    rebindings.extend(self.owned_payload_rebindings(element, element_ty));
                }
            }
            if let Some(Some(rest_name)) = rest {
                let ident = rust_ident(rest_name);
                rebindings.push((ident.clone(), format!("{ident}.to_vec()")));
            }
            return rebindings;
        }
        Vec::new()
    }

    /// The owned rebinding for a single by-ref match binding: `*x` for a `Copy`
    /// payload, `x.clone()` for any other cloneable value type, and nothing for a
    /// resource (it can't be moved out of a shared `read` view).
    pub(super) fn owned_rebinding_for(
        &self,
        bind_name: &str,
        field_ty: &TypeRef,
    ) -> Option<(String, String)> {
        let ident = rust_ident(bind_name);
        if Self::is_copy_primitive(field_ty) {
            Some((ident.clone(), format!("*{ident}")))
        } else if self.is_resource_type(field_ty) {
            None
        } else {
            Some((ident.clone(), format!("{ident}.clone()")))
        }
    }

    /// The generic type parameters declared by the type backing `pattern_name`
    /// when matched against `value_type` (struct or sum variant), in declaration
    /// order — so concrete arguments from `value_type` can be substituted in.
    pub(super) fn pattern_type_params(
        &self,
        value_type: Option<&TypeRef>,
        pattern_name: &str,
    ) -> Vec<String> {
        let Some(root) = value_type.map(|ty| ty.name.as_str()) else {
            return Vec::new();
        };
        for item in &self.program.items {
            match item {
                Item::Type(type_decl) if type_decl.name == root && pattern_name == root => {
                    return type_decl
                        .type_params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect();
                }
                Item::SumType(sum)
                    if sum.name == root && sum.variants.iter().any(|v| v.name == pattern_name) =>
                {
                    return sum
                        .type_params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect();
                }
                _ => {}
            }
        }
        Vec::new()
    }

    pub(super) fn lower_match_pattern_typed(
        &self,
        pattern: &MatchPattern,
        value_type: Option<&TypeRef>,
        by_ref: bool,
    ) -> String {
        match pattern {
            MatchPattern::Binding { name, .. } => rust_ident(name),
            MatchPattern::Wildcard(_) => "_".to_string(),
            MatchPattern::Literal { value, .. } => lower_match_literal(value),
            MatchPattern::Variant { name, binding, .. }
                if name == "Some" && value_type.is_some_and(|ty| ty.name == "Option") =>
            {
                let inner = value_type.and_then(|ty| ty.args.first());
                let payload = binding
                    .as_ref()
                    .map(|binding| self.lower_match_pattern_typed(binding, inner, by_ref))
                    .unwrap_or_else(|| "_".to_string());
                format!("Some({payload})")
            }
            MatchPattern::Variant { name, binding, .. }
                if matches!(name.as_str(), "Ok" | "Err")
                    && value_type.is_some_and(|ty| ty.name == "Result") =>
            {
                let inner = value_type.and_then(|ty| {
                    if name == "Ok" {
                        ty.args.first()
                    } else {
                        ty.args.get(1)
                    }
                });
                let payload = binding
                    .as_ref()
                    .map(|binding| self.lower_match_pattern_typed(binding, inner, by_ref))
                    .unwrap_or_else(|| "_".to_string());
                format!("{}({payload})", rust_ident(name))
            }
            MatchPattern::Variant { name, binding, .. } => {
                if let Some((sum_name, fields)) = self.sum_variant_fields_for_type(value_type, name)
                {
                    if fields.is_empty() {
                        return format!("{}::{}", rust_ident(&sum_name), rust_ident(name));
                    }
                    let mut parts = Vec::new();
                    let single_field = fields.len() == 1;
                    for field in &fields {
                        let field_pattern = if single_field {
                            binding
                                .as_ref()
                                .map(|binding| {
                                    self.lower_match_pattern_typed(binding, Some(&field.ty), by_ref)
                                })
                                .unwrap_or_else(|| "_".to_string())
                        } else {
                            "_".to_string()
                        };
                        parts.push(format!("{}: {}", rust_ident(&field.name), field_pattern));
                    }
                    return format!(
                        "{}::{} {{ {} }}",
                        rust_ident(&sum_name),
                        rust_ident(name),
                        parts.join(", ")
                    );
                }
                lower_match_pattern(pattern)
            }
            MatchPattern::Struct {
                name,
                fields,
                has_rest,
                ..
            } => self.lower_struct_match_pattern_typed(name, fields, *has_rest, value_type, by_ref),
            MatchPattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => {
                let element_type = value_type.and_then(|ty| ty.args.first());
                let mut parts: Vec<String> = prefix
                    .iter()
                    .map(|pattern| self.lower_match_pattern_typed(pattern, element_type, by_ref))
                    .collect();
                if let Some(rest_binding) = rest {
                    match rest_binding {
                        Some(name) => parts.push(format!("{} @ ..", rust_ident(name))),
                        None => parts.push("..".to_string()),
                    }
                }
                parts.extend(
                    suffix.iter().map(|pattern| {
                        self.lower_match_pattern_typed(pattern, element_type, by_ref)
                    }),
                );
                format!("[{}]", parts.join(", "))
            }
        }
    }

    pub(super) fn lower_struct_match_pattern_typed(
        &self,
        name: &str,
        fields: &[crate::syntax::ast::MatchFieldPattern],
        has_rest: bool,
        value_type: Option<&TypeRef>,
        by_ref: bool,
    ) -> String {
        let namespace = value_type.and_then(|ty| {
            self.sum_variant_fields_for_type(Some(ty), name)
                .map(|(sum_name, _)| sum_name)
        });
        let path = namespace
            .as_ref()
            .map(|sum_name| format!("{}::{}", rust_ident(sum_name), rust_ident(name)))
            .unwrap_or_else(|| rust_ident(name));
        let declared_fields = self.pattern_declared_field_types(value_type, name);
        let mut parts = Vec::new();
        for field in fields {
            if field.ignored {
                parts.push(format!("{}: _", rust_ident(&field.name)));
            } else if let Some(pattern) = &field.pattern {
                let field_type = declared_fields
                    .as_ref()
                    .and_then(|fields| fields.iter().find(|candidate| candidate.name == field.name))
                    .map(|field| &field.ty);
                parts.push(format!(
                    "{}: {}",
                    rust_ident(&field.name),
                    self.lower_match_pattern_typed(pattern, field_type, by_ref)
                ));
            } else if let Some(binding) = &field.binding {
                let binding_text = if field.effect == Some(crate::syntax::ast::DataEffect::Mut) {
                    format!("mut {}", rust_ident(binding))
                } else {
                    rust_ident(binding)
                };
                if binding == &field.name
                    && field.effect != Some(crate::syntax::ast::DataEffect::Mut)
                {
                    parts.push(rust_ident(&field.name));
                } else {
                    parts.push(format!("{}: {binding_text}", rust_ident(&field.name)));
                }
            }
        }
        if has_rest {
            parts.push("..".to_string());
        }
        format!("{path} {{ {} }}", parts.join(", "))
    }

    pub(super) fn pattern_declared_field_types(
        &self,
        value_type: Option<&TypeRef>,
        pattern_name: &str,
    ) -> Option<Vec<FieldDecl>> {
        let root = value_type?.name.as_str();
        if let Some((_, fields)) = self.sum_variant_fields_for_type(value_type, pattern_name) {
            return Some(fields);
        }
        if pattern_name == root {
            return self.program.items.iter().find_map(|item| match item {
                Item::Type(type_decl) if type_decl.name == root => Some(type_decl.fields.clone()),
                _ => None,
            });
        }
        None
    }

    pub(super) fn sum_variant_fields_for_type(
        &self,
        value_type: Option<&TypeRef>,
        variant_name: &str,
    ) -> Option<(String, Vec<FieldDecl>)> {
        let root = &value_type?.name;
        self.program.items.iter().find_map(|item| match item {
            Item::SumType(sum) if &sum.name == root => sum
                .variants
                .iter()
                .find(|variant| variant.name == variant_name)
                .map(|variant| (sum.name.clone(), variant.fields.clone())),
            _ => None,
        })
    }

    pub(super) fn find_sum_type_for_variant(&self, variant_name: &str) -> Option<String> {
        // Skip built-in variants
        if matches!(
            variant_name,
            "Some" | "None" | "Ok" | "Err" | "true" | "false"
        ) {
            return None;
        }
        for item in &self.program.items {
            if let Item::SumType(sum) = item {
                if sum.variants.iter().any(|v| v.name == variant_name) {
                    return Some(sum.name.clone());
                }
            }
        }
        None
    }
}
