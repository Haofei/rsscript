//! Core expression emission independent of host runtime dispatch.

use super::*;

impl<'a> RustLowerer<'a> {
    pub(in crate::rust_lower) fn lower_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(name, _) => {
                if self.lowering_default && self.const_names.contains(name) {
                    rust_ident(name).to_uppercase()
                } else if self.is_mut_copy_scalar_param(name) {
                    // A `mut` Copy-scalar parameter lowers to `&mut T`; a bare read
                    // dereferences it so the value (and assignment through it) works
                    // against the `&mut i64`/`&mut bool`/… binding. As an assignment
                    // target this yields `(*pos) = …`, writing back to the caller.
                    format!("(*{})", rust_value_ident(name))
                } else if self.drop_field_names.contains(name) {
                    format!("self.{}", rust_ident(name))
                } else if self.read_view_bindings.contains(name)
                    && self
                        .value_types
                        .get(name)
                        .is_some_and(|ty| !is_copy_type_ref(ty))
                {
                    format!("{}.clone()", rust_value_ident(name))
                } else if let Some(sum_name) = self.find_sum_type_for_variant(name) {
                    format!("{}::{}", rust_ident(&sum_name), rust_ident(name))
                } else {
                    lower_builtin_value_ident(name)
                        .map(str::to_string)
                        .unwrap_or_else(|| rust_value_ident(name))
                }
            }
            // Integer literals lower as `i64` (RSScript `Int` is i64); without the
            // suffix Rust infers `i32` for an all-literal sub-expression and can
            // const-overflow at compile time even when the i64 value fits. Float
            // literals already default to `f64` (RSScript `Float`), so leave them.
            Expr::Number(value, _) => {
                if value.contains('.') {
                    value.clone()
                } else {
                    format!("{value}i64")
                }
            }
            Expr::String(value, _) => format!("{:?}.to_string()", decode_string_token(value)),
            // A `char` is Copy: emit the bare Rust char literal (e.g. `'a'`,
            // `'\n'`, `'\''`) with NO trailing `.to_string()` (that would change
            // the type to String and break interpreter/native/AOT parity).
            Expr::CharLiteral(value, _) => format!("{:?}", decode_char_token(value)),
            Expr::MultilineString(value, _) => format!("{value:?}.to_string()"),
            Expr::ObjectLiteral { .. } => self.lower_json_value(expr),
            Expr::MapLiteral { span, .. } => unreachable_lowering("map literal", span),
            Expr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_owned_expr(item))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("vec![{items}]")
            }
            Expr::Binary {
                op, left, right, ..
            } => {
                let binary_op = *op;
                // Flatten a deep `&&`/`||` chain into a *balanced* tree so the
                // generated Rust nests O(log n) deep instead of O(n): a left-linear
                // chain of hundreds of `&&` overflows rustc's recursive parser/
                // type-checker (the `RUST_MIN_STACK=2g` workaround). `&&`/`||` are
                // associative for both value AND short-circuit/evaluation order, so
                // balanced regrouping is behavior-identical. The chain is collected
                // iteratively (its left spine is the deep part) so this lowerer pass
                // does not itself recurse n-deep.
                if matches!(binary_op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) {
                    let mut rev: Vec<&Expr> = vec![right.as_ref()];
                    let mut node: &Expr = left.as_ref();
                    loop {
                        match node {
                            Expr::Binary {
                                op: inner,
                                left: inner_left,
                                right: inner_right,
                                ..
                            } if *inner == binary_op => {
                                rev.push(inner_right.as_ref());
                                node = inner_left.as_ref();
                            }
                            other => {
                                rev.push(other);
                                break;
                            }
                        }
                    }
                    if rev.len() >= 8 {
                        rev.reverse();
                        let op_str = if binary_op == BinaryOp::LogicalAnd {
                            "&&"
                        } else {
                            "||"
                        };
                        let leaves: Vec<String> = rev
                            .iter()
                            .map(|operand| self.lower_binary_operand(operand, binary_op, false))
                            .collect();
                        return balanced_logical_join(&leaves, op_str);
                    }
                }
                if matches!(op, BinaryOp::ShiftLeft | BinaryOp::ShiftRight) {
                    let helper = if *op == BinaryOp::ShiftLeft {
                        "int_shift_left"
                    } else {
                        "int_shift_right"
                    };
                    let left = self.lower_expr(left);
                    let right = self.lower_expr(right);
                    return format!("rsscript_runtime::{helper}({left} as i64, {right} as i64)");
                }
                let op = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Subtract => "-",
                    BinaryOp::Multiply => "*",
                    BinaryOp::Divide => "/",
                    BinaryOp::Modulo => "%",
                    BinaryOp::BitAnd => "&",
                    BinaryOp::BitOr => "|",
                    BinaryOp::BitXor => "^",
                    BinaryOp::ShiftLeft | BinaryOp::ShiftRight => unreachable!(),
                    BinaryOp::Equal => "==",
                    BinaryOp::NotEqual => "!=",
                    BinaryOp::Less => "<",
                    BinaryOp::LessEqual => "<=",
                    BinaryOp::Greater => ">",
                    BinaryOp::GreaterEqual => ">=",
                    BinaryOp::LogicalAnd => "&&",
                    BinaryOp::LogicalOr => "||",
                };
                if matches!(op, "==" | "!=")
                    && (self.is_string_comparison_operand(left)
                        || self.is_string_comparison_operand(right))
                {
                    return format!(
                        "{} {op} {}",
                        self.lower_string_comparison_operand(left),
                        self.lower_string_comparison_operand(right)
                    );
                }
                if matches!(op, "==" | "!=") {
                    // A `read`-bound enum parameter lowers to `&Op`; comparing it
                    // against a sum *value* (e.g. a bare variant) needs a deref so
                    // both sides are `Op`. Mirrors how field-access enum comparison
                    // already lowers to a value on both sides.
                    let left_ref = self.is_enum_read_ref_operand(left);
                    let right_ref = self.is_enum_read_ref_operand(right);
                    if left_ref && !right_ref && self.is_enum_value_operand(right) {
                        return format!(
                            "*{} {op} {}",
                            self.lower_binary_operand(left, binary_op, false),
                            self.lower_binary_operand(right, binary_op, true)
                        );
                    }
                    if right_ref && !left_ref && self.is_enum_value_operand(left) {
                        return format!(
                            "{} {op} *{}",
                            self.lower_binary_operand(left, binary_op, false),
                            self.lower_binary_operand(right, binary_op, true)
                        );
                    }
                }
                format!(
                    "{} {op} {}",
                    self.lower_binary_operand(left, binary_op, false),
                    self.lower_binary_operand(right, binary_op, true)
                )
            }
            Expr::Field { base, name, span } => {
                if self
                    .infer_expr_type(base)
                    .is_some_and(|ty| self.is_class_type(&ty))
                    || self.expr_lowers_to_managed_non_class_handle(base)
                {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_read_at({})).{}.clone()",
                        self.lower_expr(base),
                        lower_source_span(span),
                        rust_ident(name)
                    )
                } else if self.expr_is_read_view(base) {
                    let lowered = format!(
                        "{}.{}",
                        self.lower_read_view_base_expr(base),
                        rust_ident(name)
                    );
                    if self
                        .infer_expr_type(expr)
                        .is_some_and(|ty| !is_copy_type_ref(&ty))
                    {
                        format!("{lowered}.clone()")
                    } else {
                        lowered
                    }
                } else {
                    format!("{}.{}", self.lower_expr(base), rust_ident(name))
                }
            }
            Expr::Index { base, index, .. } => {
                format!(
                    "{}[rsscript_runtime::checked_list_index({})]",
                    self.lower_expr(base),
                    self.lower_expr(index)
                )
            }
            Expr::Call { callee, args, span } => {
                // Thin router: each `lower_call_*` helper recognizes one family of
                // call shapes and returns `Some(rust)` if it applies; the final
                // `lower_call_dispatch` handles the generic/fallthrough call forms.
                if let Some(lowered) = self.lower_call_json_codec(callee, args, span) {
                    return lowered;
                }
                if let Some(lowered) = self.lower_call_task_cancellation_token(callee) {
                    return lowered;
                }
                if let Some(lowered) = self.lower_call_named_constructor(callee, args, span) {
                    return lowered;
                }
                if let Some(lowered) = self.lower_bound_call(callee, args, span) {
                    return lowered;
                }
                self.lower_call_after_binding(callee, args, span)
            }
            Expr::Effect {
                effect,
                value,
                span,
            } => match effect {
                DataEffect::Read => {
                    if self.expr_is_read_view(value) {
                        self.lower_read_view_expr(value)
                    } else if self.expr_lowers_to_managed_non_class_handle(value) {
                        format!(
                            "&*rsscript_runtime::unwrap_runtime({}.try_read_at({}))",
                            self.lower_expr(value),
                            lower_source_span(span)
                        )
                    } else {
                        format!("&({})", self.lower_expr(value))
                    }
                }
                DataEffect::Mut => {
                    if let Expr::Ident(name, _) = &**value
                        && self.param_effects.get(name) == Some(&DataEffect::Mut)
                    {
                        rust_value_ident(name)
                    } else if self
                        .infer_expr_type(value)
                        .is_some_and(|ty| self.is_class_type(&ty))
                    {
                        format!("&{}", self.lower_expr(value))
                    } else if self.expr_lowers_to_managed_non_class_handle(value) {
                        format!(
                            "&mut *rsscript_runtime::unwrap_runtime({}.try_write_at({}))",
                            self.lower_expr(value),
                            lower_source_span(span)
                        )
                    } else {
                        format!("&mut {}", self.lower_expr(value))
                    }
                }
                DataEffect::Take => self.lower_expr(value),
            },
            Expr::Manage { value, span } => {
                format!(
                    "rsscript_runtime::manage_at({}, {})",
                    self.lower_expr(value),
                    lower_source_span(span)
                )
            }
            Expr::Spawn { span, .. } => unreachable_lowering("spawn expression", span),
            Expr::Await { value, .. } => self.lower_await_expr(value),
            Expr::Try { value, .. } => format!("{}?", self.lower_expr(value)),
            Expr::Closure { params, body, .. } => {
                let lowered_params = params
                    .iter()
                    .map(|param| rust_ident(param))
                    .collect::<Vec<_>>()
                    .join(", ");
                let previous_return_type = self.current_return_type.take();
                if let [Stmt::Expr(value)] = body.statements.as_slice() {
                    let lowered = format!("|{lowered_params}| {}", self.lower_expr(value));
                    self.current_return_type = previous_return_type;
                    return lowered;
                }
                let mut out = String::new();
                out.push_str(&format!("|{lowered_params}| {{\n"));
                self.lower_block(body, &mut out, 1);
                out.push('}');
                self.current_return_type = previous_return_type;
                out
            }
            Expr::Match { value, arms, .. } => {
                let scrutinee_type = self
                    .infer_expr_type(value)
                    .map(|ty| self.canonical_type_ref(&ty));
                let mut scrutinee = self.lower_match_scrutinee_expr(value, scrutinee_type.as_ref());
                let by_ref = self.match_scrutinee_by_ref(value);
                if arms_have_list_pattern(arms) {
                    scrutinee = format!("({scrutinee}).as_slice()");
                }
                let mut out = format!("match {scrutinee} {{\n");
                for arm in arms {
                    let pattern = self.lower_match_pattern_typed(
                        &arm.pattern,
                        scrutinee_type.as_ref(),
                        by_ref,
                    );
                    let guard = arm
                        .guard
                        .as_ref()
                        .map(|guard| format!(" if {}", self.lower_expr(guard)))
                        .unwrap_or_default();
                    let mut body = self.lower_expr_block(&arm.body, 1);
                    if by_ref || matches!(arm.pattern, MatchPattern::List { .. }) {
                        let binds =
                            self.owned_payload_rebindings(&arm.pattern, scrutinee_type.as_ref());
                        if !binds.is_empty() {
                            let rebinds = binds
                                .iter()
                                .map(|(n, rhs)| format!("let {n} = {rhs};"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            body = body.replacen('{', &format!("{{ {rebinds}"), 1);
                        }
                    }
                    out.push_str(&format!("    {pattern}{guard} => {body}"));
                    out.push_str(",\n");
                }
                out.push('}');
                out
            }
            Expr::Unknown(span) => unreachable_lowering("expression", span),
        }
    }

    pub(in crate::rust_lower) fn lower_default_parameter_helpers(&mut self, out: &mut String) {
        let defaults = self.function_param_defaults.clone();
        self.lowering_default = true;
        for (function, values) in defaults {
            let Some(types) = self.function_param_types.get(&function).cloned() else {
                continue;
            };
            let helpers = self
                .function_param_default_helpers
                .get(&function)
                .cloned()
                .unwrap_or_default();
            for (index, default) in values.into_iter().enumerate() {
                let (Some(default), Some(Some(helper)), Some((_, ty))) =
                    (default, helpers.get(index), types.get(index))
                else {
                    continue;
                };
                let return_type = self.lower_type_ref(ty, ManagedPosition::Bare);
                let value = self.lower_expr_for_expected_type(&default, ty);
                out.push_str(&format!(
                    "#[inline]\nfn {helper}() -> {return_type} {{ {value} }}\n\n"
                ));
            }
        }
        self.lowering_default = false;
    }
}
