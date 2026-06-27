use crate::diagnostic::Span;
use crate::syntax::ast::{Callee, DataEffect, Expr, Item, TypeKind, TypeRef};

use super::helpers::*;

use super::lowerer::*;

impl RustLowerer<'_> {
    pub(super) fn expr_is_read_view(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => self.read_view_bindings.contains(name),
            Expr::Field { base, .. } => self.expr_is_read_view(base),
            Expr::Effect { value, .. } | Expr::Try { value, .. } => self.expr_is_read_view(value),
            _ => false,
        }
    }

    pub(super) fn lower_read_view_base_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(name, _) if self.read_view_bindings.contains(name) => {
                rust_value_ident(name)
            }
            _ => self.lower_expr(expr),
        }
    }

    pub(super) fn lower_read_view_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Ident(name, _) if self.read_view_bindings.contains(name) => {
                rust_value_ident(name)
            }
            Expr::Field { base, name, .. } if self.expr_is_read_view(base) => {
                format!(
                    "&{}.{}",
                    self.lower_read_view_base_expr(base),
                    rust_ident(name)
                )
            }
            _ => format!("&{}", self.lower_expr(expr)),
        }
    }

    /// Apply a `read`/`mut`/`take` effect to a managed-handle argument: `read`
    /// borrows through the handle's read view, `mut` takes `&mut` of the lowered
    /// value, and `take` moves the lowered value.
    pub(super) fn lower_managed_handle_effect_arg(
        &mut self,
        effect: DataEffect,
        value: &Expr,
    ) -> String {
        match effect {
            DataEffect::Read => self.lower_managed_read_ref(value),
            DataEffect::Mut => format!("&mut {}", self.lower_expr(value)),
            DataEffect::Take => self.lower_expr(value),
        }
    }

    /// Whether the callee is a first-class closure VALUE (a local binding whose
    /// inferred type is an `owned Fn(...)`), rather than a function/constructor.
    /// Such a call dispatches through the stored `Rc<dyn Fn>`.
    pub(super) fn callee_is_closure_value(&self, callee: &Callee) -> bool {
        let Callee::Name(name) = callee else {
            return false;
        };
        // A real function/constructor takes precedence over a same-named local.
        if self.function_param_types.contains_key(type_root_name(name)) {
            return false;
        }
        self.value_types
            .get(name)
            .is_some_and(|ty| ty.name == "Fn" && ty.is_owned)
    }

    /// The declared data effect of the `index`-th parameter of a first-class
    /// closure value's stored `Fn` type, so a call site can pass the argument
    /// with the matching Rust ABI (`read` -> `&`, `mut` -> `&mut`).
    pub(super) fn closure_value_param_effect(
        &self,
        callee: &Callee,
        index: usize,
    ) -> Option<DataEffect> {
        let Callee::Name(name) = callee else {
            return None;
        };
        let ty = self.value_types.get(name)?;
        if ty.name != "Fn" {
            return None;
        }
        ty.fn_param_effects.get(index).copied().flatten()
    }

    // Lower a `read`-effect managed-handle argument to a `&T`. A managed `let` local lowers to
    // an owned `T`, so it needs a leading `&`. A managed `read`-PARAM already lowers to `&T`
    // (its Rust param type is a reference), so adding another `&` produced `&&T` and an
    // ill-typed push into an owned collection (RS1101/RS1102 E0308). Detect the already-ref
    // case and don't double-borrow.
    pub(super) fn lower_managed_read_ref(&mut self, value: &Expr) -> String {
        if let Expr::Ident(name, _) = value
            && matches!(self.param_effects.get(name), Some(DataEffect::Read))
        {
            return self.lower_expr(value);
        }
        format!("&{}", self.lower_expr(value))
    }

    /// True when `expr` is exactly a `read`-effect parameter ident whose type
    /// lowers to a plain `&T` borrow (managed / non-Copy) — i.e. binding it to a
    /// `let mut` local would yield `&T` and break a later owned reassignment.
    /// Excludes Copy scalars (passed by value), retained `Managed<T>` params
    /// (which lower to `&Managed<T>`, not a plain `&T`), and `read`-view binds
    /// (already cloned at use sites).
    pub(super) fn let_init_is_clonable_read_param_ref(&self, expr: &Expr) -> bool {
        let Expr::Ident(name, _) = expr else {
            return false;
        };
        if self.param_effects.get(name) != Some(&DataEffect::Read) {
            return false;
        }
        if self.read_view_bindings.contains(name) {
            return false;
        }
        let Some(ty) = self.value_types.get(name) else {
            return false;
        };
        // Copy scalars are passed `read` by value, so the local already owns a `T`.
        if is_copy_type_ref(ty) || Self::read_effect_lowers_by_value(ty) {
            return false;
        }
        // Retained non-class params lower to `&Managed<T>`, not a plain `&T`;
        // cloning that would clone the `Managed` handle, not produce an owned `T`.
        if self.current_retained_params.contains(name)
            && !self.is_class_type(ty)
            && self.type_kinds.contains_key(&ty.name)
        {
            return false;
        }
        true
    }

    pub(super) fn expr_lowers_to_managed_handle(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => self.managed_bindings.contains(name),
            Expr::Field { base, name, .. } => self
                .infer_expr_type(base)
                .is_some_and(|base_ty| self.is_runtime_handle_field(&base_ty.name, name)),
            Expr::Manage { .. } => true,
            Expr::Call {
                callee: Callee::Name(name),
                ..
            } => self
                .type_kinds
                .get(name)
                .is_some_and(|kind| *kind == TypeKind::Class),
            Expr::Effect { value, .. } | Expr::Try { value, .. } => {
                self.expr_lowers_to_managed_handle(value)
            }
            _ => false,
        }
    }

    pub(super) fn expr_lowers_to_managed_non_class_handle(&self, expr: &Expr) -> bool {
        self.expr_lowers_to_managed_handle(expr)
            && !self
                .infer_expr_type(expr)
                .is_some_and(|ty| self.is_class_type(&ty))
    }

    /// True when this operand is a `read`-bound parameter whose type is a
    /// user-defined sum type. Such a parameter lowers to `&Op`, so comparing it
    /// against a sum *value* (e.g. a bare variant literal) needs a deref.
    pub(super) fn is_enum_read_ref_operand(&self, expr: &Expr) -> bool {
        let Expr::Ident(name, _) = expr else {
            return false;
        };
        if self.param_effects.get(name) != Some(&DataEffect::Read) {
            return false;
        }
        let Some(ty) = self.value_types.get(name) else {
            return false;
        };
        ty.args.is_empty() && self.is_sum_type_name(&ty.name)
    }

    /// True when this operand is a sum *value* that lowers by value (not behind a
    /// reference) — most commonly a bare variant literal such as `A`.
    pub(super) fn is_enum_value_operand(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Ident(name, _) => self.find_sum_type_for_variant(name).is_some(),
            _ => false,
        }
    }

    pub(super) fn is_sum_type_name(&self, name: &str) -> bool {
        self.program
            .items
            .iter()
            .any(|item| matches!(item, Item::SumType(sum) if sum.name == name))
    }

    // Whether a user-declared struct/sum lowers to a Rust type that derives `Clone`. Mirrors
    // `compute_derive_attr`: an omitted derive list defaults to Debug+Clone (non-resource), and an
    // explicit list is cloneable only if it includes `Clone`.
    pub(super) fn type_derives_clone(&self, name: &str) -> bool {
        self.program.items.iter().any(|item| match item {
            Item::Type(decl) => {
                decl.name == name
                    && (decl.derives.iter().any(|d| d == "Clone")
                        || (decl.derives.is_empty() && decl.kind != TypeKind::Resource))
            }
            Item::SumType(sum) => {
                sum.name == name
                    && (sum.derives.is_empty() || sum.derives.iter().any(|d| d == "Clone"))
            }
            _ => false,
        })
    }

    // The Copy scalar primitives — payloads of these match-bind by value (with a deref-pattern
    // when the scrutinee is a borrow). Distinct from `read_effect_lowers_by_value` (intrinsic-ABI tuned).
    pub(super) fn is_copy_primitive(ty: &TypeRef) -> bool {
        ty.args.is_empty()
            && matches!(
                ty.name.as_str(),
                "Bool"
                    | "Byte"
                    | "Char"
                    | "Int"
                    | "Int8"
                    | "Int16"
                    | "Int32"
                    | "Int64"
                    | "UInt"
                    | "UInt8"
                    | "UInt16"
                    | "UInt32"
                    | "UInt64"
                    | "Float"
                    | "Float32"
                    | "Float64"
            )
    }

    pub(super) fn read_effect_lowers_by_value(expected: &TypeRef) -> bool {
        // Whether a `read`-effect param/arg of this type lowers by value vs `&T`.
        // Tuned to the runtime intrinsic ABI (receiver methods like
        // `char_is_alpha`/`float_to_string` take `&char`/`&f64`), so `Char`/`Float`
        // stay by-reference here; user-function by-value Copy params are handled
        // separately in `lower_call_arg_for_callee`.
        expected.args.is_empty()
            && matches!(
                expected.name.as_str(),
                "Bool"
                    | "Byte"
                    | "Int"
                    | "Int8"
                    | "Int16"
                    | "Int32"
                    | "Int64"
                    | "UInt"
                    | "UInt8"
                    | "UInt16"
                    | "UInt32"
                    | "UInt64"
            )
    }

    pub(super) fn is_weak_field(&self, type_name: &str, field_name: &str) -> bool {
        self.program.items.iter().any(|item| match item {
            Item::Type(ty) if ty.name == type_name => ty
                .fields
                .iter()
                .any(|field| field.name == field_name && field.is_weak),
            _ => false,
        })
    }

    pub(super) fn is_runtime_handle_field(&self, type_name: &str, field_name: &str) -> bool {
        self.program.items.iter().any(|item| match item {
            Item::Type(ty) if ty.name == type_name => ty.fields.iter().any(|field| {
                field.name == field_name
                    && !field.is_weak
                    && (field.is_handle
                        || matches!(self.type_kinds.get(&field.ty.name), Some(TypeKind::Class)))
            }),
            _ => false,
        })
    }

    pub(super) fn lower_runtime_handle_field_value(
        &mut self,
        type_name: &str,
        field_name: &str,
        expr: &Expr,
        span: &Span,
    ) -> String {
        let value = self.lower_owned_expr(expr);
        if self
            .field_type(type_name, field_name)
            .is_some_and(|ty| self.is_class_type(&ty))
        {
            value
        } else {
            format!(
                "rsscript_runtime::manage_at({value}, {})",
                lower_source_span(span)
            )
        }
    }

    pub(super) fn lower_explicit_weak_field_value(&mut self, expr: &Expr) -> String {
        if let Some(value) = explicit_weak_handle_source(expr) {
            return self.lower_runtime_weak_from_managed(value);
        }
        self.lower_expr(expr)
    }

    pub(super) fn lower_runtime_weak_from_managed(&mut self, expr: &Expr) -> String {
        if let Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } = expr
        {
            let value_expr = self.lower_expr(value);
            if let Expr::Ident(name, _) = &**value
                && matches!(
                    self.param_effects.get(name),
                    Some(DataEffect::Read | DataEffect::Mut)
                )
            {
                return format!("rsscript_runtime::weak({value_expr})");
            }
            return format!("rsscript_runtime::weak(&{value_expr})");
        }
        format!("rsscript_runtime::weak(&{})", self.lower_expr(expr))
    }
}
