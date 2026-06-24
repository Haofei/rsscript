
use crate::syntax::ast::{
    BinaryOp, Callee, DataEffect, Expr, Stmt, TypeKind, TypeRef,
};

use super::helpers::*;

use super::lowerer::*;

impl RustLowerer<'_> {
    pub(super) fn type_ref_is_concrete_for_annotation(&self, ty: &TypeRef) -> bool {
        let root_is_concrete = matches!(
            ty.name.as_str(),
            "Unit"
                | "Bool"
                | "Int"
                | "Float"
                | "String"
                | "JsonValue"
                | "Path"
                | "List"
                | "Map"
                | "Set"
                | "Option"
                | "Result"
        ) || self.type_kinds.contains_key(&ty.name)
            || capability_protocol_name(&ty.name).is_some();
        root_is_concrete
            && ty
                .args
                .iter()
                .all(|arg| self.type_ref_is_concrete_for_annotation(arg))
            && ty
                .fn_params
                .iter()
                .all(|param| self.type_ref_is_concrete_for_annotation(param))
            && ty
                .fn_return
                .as_deref()
                .is_none_or(|return_ty| self.type_ref_is_concrete_for_annotation(return_ty))
    }


    pub(super) fn infer_call_arg_type(&self, expr: &Expr) -> Option<TypeRef> {
        match expr {
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => self.infer_call_arg_type(value),
            _ => self.infer_expr_type(expr),
        }
    }


    pub(super) fn infer_expr_type(&self, expr: &Expr) -> Option<TypeRef> {
        match expr {
            Expr::Ident(name, span) if name == "true" || name == "false" => {
                Some(simple_type_ref("Bool", span))
            }
            Expr::Ident(name, span) if name == "null" => Some(simple_type_ref("JsonLiteral", span)),
            Expr::Ident(name, span) => self.value_types.get(name).cloned().or_else(|| {
                self.find_sum_type_for_variant(name)
                    .map(|sum_name| simple_type_ref(&sum_name, span))
            }),
            Expr::Number(value, span) => Some(simple_type_ref(
                crate::hir::number_literal_type_name(value),
                span,
            )),
            Expr::String(_, span) => Some(simple_type_ref("String", span)),
            Expr::Binary { op, span, .. } => {
                let name = match op {
                    BinaryOp::Add
                    | BinaryOp::Subtract
                    | BinaryOp::Multiply
                    | BinaryOp::Divide
                    | BinaryOp::Modulo
                    | BinaryOp::BitAnd
                    | BinaryOp::BitOr
                    | BinaryOp::BitXor
                    | BinaryOp::ShiftLeft
                    | BinaryOp::ShiftRight => "Int",
                    BinaryOp::Equal
                    | BinaryOp::NotEqual
                    | BinaryOp::Less
                    | BinaryOp::LessEqual
                    | BinaryOp::Greater
                    | BinaryOp::GreaterEqual
                    | BinaryOp::LogicalAnd
                    | BinaryOp::LogicalOr => "Bool",
                };
                Some(simple_type_ref(name, span))
            }
            Expr::Field { base, name, span } => {
                let base_ty = self.infer_expr_type(base)?;
                self.field_type(&base_ty.name, name).map(|ty| TypeRef {
                    span: span.clone(),
                    ..ty
                })
            }
            Expr::Call {
                callee: Callee::Name(name),
                span,
                ..
            } if self.type_kinds.contains_key(name) => Some(simple_type_ref(name, span)),
            Expr::Call {
                callee: Callee::Qualified { namespace, name },
                span,
                ..
            } if type_root_name(name) == "new" && type_arg_names(namespace).is_some() => {
                Some(type_ref_from_display(namespace, span))
            }
            Expr::Call { callee, span, .. } if capability_from_protocol(callee).is_some() => {
                let protocol = capability_from_protocol(callee)?;
                Some(TypeRef {
                    args: vec![simple_type_ref(protocol, span)],
                    ..simple_type_ref("Capability", span)
                })
            }
            Expr::Call {
                callee, args, span, ..
            } if let Callee::Name(name) = callee => self
                .value_types
                .get(name)
                .and_then(fn_type_return)
                .cloned()
                .or_else(|| self.infer_call_return_type(callee, args, span)),
            Expr::Call {
                callee:
                    Callee::ReceiverCall {
                        receiver, method, ..
                    },
                args,
                span,
                ..
            } => {
                let receiver_type = self.infer_expr_type(receiver)?;
                let namespace = self.receiver_call_namespace(&receiver_type, method);
                self.infer_call_return_type(
                    &Callee::Qualified {
                        namespace,
                        name: method.clone(),
                    },
                    args,
                    span,
                )
            }
            Expr::Call { callee, args, span } => self.infer_call_return_type(callee, args, span),
            Expr::ObjectLiteral { span, .. } => Some(simple_type_ref("JsonLiteral", span)),
            Expr::MapLiteral { span, .. } => Some(simple_type_ref("MapLiteral", span)),
            Expr::ArrayLiteral { items, span } => {
                let item_ty = items.first().and_then(|item| self.infer_expr_type(item));
                Some(TypeRef {
                    args: item_ty.into_iter().collect(),
                    ..simple_type_ref("List", span)
                })
            }
            Expr::Effect { value, .. } => self.infer_expr_type(value),
            Expr::Manage { value, .. } => self.infer_expr_type(value),
            Expr::Try { value, .. } => self
                .infer_expr_type(value)
                .and_then(|ty| result_ok_type_ref(&ty)),
            Expr::Match { arms, .. } => arms.first().and_then(|arm| {
                arm.body
                    .statements
                    .iter()
                    .next_back()
                    .and_then(|statement| match statement {
                        Stmt::Return(stmt) => stmt
                            .value
                            .as_ref()
                            .and_then(|value| self.infer_expr_type(value)),
                        Stmt::Expr(value) => self.infer_expr_type(value),
                        _ => None,
                    })
            }),
            _ => None,
        }
    }


    /// The Rust type annotation to emit on a `let`, when it is needed for
    /// inference and provably matches the value's owned lowered type.
    ///
    /// We only annotate the builtin generic containers (`Channel<T>`, `List<T>`,
    /// …). For those, `lower_type_ref` produces a concrete type identical to what
    /// the constructor lowers to, so the annotation is sound and resolves cases
    /// where a generic param is otherwise unconstrained (e.g.
    /// `let ch: Channel<Int> = Channel.bounded(capacity: 1)?` → `RssChannel<_>`).
    /// User types and transparent aliases are intentionally skipped: aliases are
    /// not resolved here and class types are not wrapped at this position, so
    /// annotating them could diverge from the value's actual Rust type.
    pub(super) fn lower_let_annotation(&self, ty: &TypeRef) -> Option<String> {
        const GENERIC_CONTAINERS: &[&str] = &[
            "Channel",
            "Sender",
            "Receiver",
            "Stream",
            "List",
            "Map",
            "Set",
            "Deque",
            "SortedMap",
            "SortedSet",
            "Option",
            "Result",
            "ResourcePool",
            "Capability",
        ];
        if ty.args.is_empty() || !GENERIC_CONTAINERS.contains(&ty.name.as_str()) {
            return None;
        }
        Some(self.lower_type_ref(ty, ManagedPosition::Bare))
    }


    pub(super) fn lower_type_ref(&self, ty: &TypeRef, position: ManagedPosition) -> String {
        if ty.name == "Fn" {
            // A `Fn`-type parameter's data effect determines how the parameter is
            // PASSED at the Rust call boundary: `read T` -> `&T` (shared borrow),
            // `mut T` -> `&mut T` (exclusive borrow, mutation propagates back),
            // and an omitted effect keeps the value-model default (by value). This
            // mirrors how regular fn params lower and is what makes a stored
            // `Rc<dyn Fn(&UOp, &mut Ctx) -> ..>` rule able to mutate `mut Ctx`.
            let params = ty
                .fn_params
                .iter()
                .enumerate()
                .map(|(index, param)| {
                    let lowered = self.lower_type_ref(param, ManagedPosition::Param);
                    match ty.fn_param_effects.get(index).copied().flatten() {
                        Some(DataEffect::Read) => format!("&{lowered}"),
                        Some(DataEffect::Mut) => format!("&mut {lowered}"),
                        Some(DataEffect::Take) | None => lowered,
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let return_ty = ty.fn_return.as_ref().map(|return_ty| {
                format!(
                    " -> {}",
                    self.lower_type_ref(return_ty, ManagedPosition::Return)
                )
            });
            let return_ty = return_ty.unwrap_or_default();
            // `owned Fn` in a STORABLE position (struct field, collection
            // element, binding, return) is a first-class value. It lowers to
            // `Rc<dyn Fn(...)>`:
            //   - `Rc` is `Clone`, so it satisfies `list_get<T: Clone>`,
            //     `#[derive(Clone)]` on a struct holding the closure, and the
            //     `Map<K, V>` memo — `Box<dyn Fn>` is NOT `Clone` and would fail
            //     all three.
            //   - `Rc<dyn Fn>: Fn` via `Deref`, so a closure fetched behind a
            //     shared read (`let r = List.get(read rules, i); (r.fxn)(..)`)
            //     is callable through that shared reference — no `&mut` needed
            //     (which `FnMut` would have required and which a shared `List`
            //     read cannot give).
            // A direct `owned Fn` PARAMETER keeps the existing `impl FnMut`
            // surface (it is consumed in-place, not stored). `noescape Fn` is
            // parameter-only (rejected elsewhere) and keeps its prior lowering.
            if ty.is_owned && position != ManagedPosition::Param {
                return format!("std::rc::Rc<dyn Fn({params}){return_ty}>");
            }
            return match (ty.is_noescape || ty.is_owned, position) {
                (true, ManagedPosition::Param) => {
                    format!("impl FnMut({params}){return_ty}")
                }
                (true, _) => format!("Box<dyn FnMut({params}){return_ty}>"),
                (false, ManagedPosition::Param) => format!("dyn Fn({params}){return_ty}"),
                (false, _) => format!("Box<dyn Fn({params}){return_ty}>"),
            };
        }
        let lowered = match ty.name.as_str() {
            "Unit" => "()".to_string(),
            "Bool" => "bool".to_string(),
            "Byte" => "u8".to_string(),
            "Char" => "char".to_string(),
            "Int" => "i64".to_string(),
            "Int8" => "i8".to_string(),
            "Int16" => "i16".to_string(),
            "Int32" => "i32".to_string(),
            "Int64" => "i64".to_string(),
            "UInt" => "u64".to_string(),
            "UInt8" => "u8".to_string(),
            "UInt16" => "u16".to_string(),
            "UInt32" => "u32".to_string(),
            "UInt64" => "u64".to_string(),
            "Float" => "f64".to_string(),
            "Float32" => "f32".to_string(),
            "Float64" => "f64".to_string(),
            "String" => "String".to_string(),
            "StringView" if position == ManagedPosition::Nested => "&str".to_string(),
            "StringView" if position == ManagedPosition::Return => "&str".to_string(),
            "StringView" => "str".to_string(),
            "StringBuilder" => "String".to_string(),
            "Url" => "String".to_string(),
            "Fd" => "i64".to_string(),
            "BytesView" | "BufferView" if position == ManagedPosition::Nested => {
                "&[u8]".to_string()
            }
            "BytesView" | "BufferView" if position == ManagedPosition::Return => {
                "&[u8]".to_string()
            }
            "BytesView" | "BufferView" => "[u8]".to_string(),
            "Bytes" | "Buffer" => "Vec<u8>".to_string(),
            "Path" => "std::path::PathBuf".to_string(),
            "Cache" if !self.type_kinds.contains_key("Cache") => {
                "rsscript_runtime::Cache".to_string()
            }
            "Rule" if !self.type_kinds.contains_key("Rule") => "rsscript_runtime::Rule".to_string(),
            "Config" if !self.type_kinds.contains_key("Config") => {
                "rsscript_runtime::Config".to_string()
            }
            "GlobalConfig" if !self.type_kinds.contains_key("GlobalConfig") => {
                "rsscript_runtime::GlobalConfig".to_string()
            }
            "Environment" => "rsscript_runtime::Environment".to_string(),
            "FunctionObject" => "rsscript_runtime::FunctionObject".to_string(),
            "Counter" => "rsscript_runtime::Counter".to_string(),
            "Instant" => "rsscript_runtime::RssInstant".to_string(),
            "Duration" => "rsscript_runtime::RssDuration".to_string(),
            "Deadline" => "rsscript_runtime::RssDeadline".to_string(),
            "CancellationSource" => "rsscript_runtime::RssCancellationSource".to_string(),
            "CancellationToken" => "rsscript_runtime::RssCancellationToken".to_string(),
            "Channel" if ty.args.len() == 1 => format!(
                "rsscript_runtime::RssChannel<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "Sender" if ty.args.len() == 1 => format!(
                "rsscript_runtime::RssSender<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "Receiver" if ty.args.len() == 1 => format!(
                "rsscript_runtime::RssReceiver<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "Stream" if ty.args.len() == 1 => format!(
                "rsscript_runtime::RssStream<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "Pipeline" if ty.args.len() == 1 => format!(
                "rsscript_runtime::RssPipeline<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "FalliblePipeline" if ty.args.len() == 2 => format!(
                "rsscript_runtime::RssFalliblePipeline<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "ChannelError" => "rsscript_runtime::ChannelError".to_string(),
            "Tensor" => "rsscript_runtime::RssTensor".to_string(),
            "TensorError" => "rsscript_runtime::TensorError".to_string(),
            "TcpStream" => "rsscript_runtime::RssTcpStream".to_string(),
            "TcpError" => "rsscript_runtime::TcpError".to_string(),
            "WebSocket" => "rsscript_runtime::RssWebSocket".to_string(),
            "WebSocketError" => "rsscript_runtime::WebSocketError".to_string(),
            "PoolStats" => "rsscript_runtime::PoolStats".to_string(),
            "PoolError" => "rsscript_runtime::PoolError".to_string(),
            "Regex" => "rsscript_runtime::RssRegex".to_string(),
            "RegexError" => "rsscript_runtime::RegexError".to_string(),
            "TempDir" => "rsscript_runtime::TempDir".to_string(),
            "File" => "rsscript_runtime::File".to_string(),
            "FileMetadata" => "rsscript_runtime::FileMetadata".to_string(),
            "FileError" => "rsscript_runtime::FileError".to_string(),
            "IOError" => "std::io::Error".to_string(),
            "ProcessEnv" => "rsscript_runtime::ProcessEnv".to_string(),
            "ProcessEvent" => "rsscript_runtime::ProcessEvent".to_string(),
            "ProcessOutput" => "rsscript_runtime::ProcessOutput".to_string(),
            "ProcessRequest" => "rsscript_runtime::ProcessRequest".to_string(),
            "Request" => "rsscript_runtime::Request".to_string(),
            "HttpRequest" => "rsscript_runtime::HttpRequest".to_string(),
            "Response" => "rsscript_runtime::Response".to_string(),
            "HttpResponse" => "rsscript_runtime::Response".to_string(),
            "HttpError" => "rsscript_runtime::HttpError".to_string(),
            "TimerError" => "rsscript_runtime::TimerError".to_string(),
            "ConfigValue" => "rsscript_runtime::ConfigValue".to_string(),
            "ConfigStore" => "rsscript_runtime::ConfigStore".to_string(),
            "ConfigError" => "rsscript_runtime::ConfigError".to_string(),
            "DbConnection" => "rsscript_runtime::DbConnection".to_string(),
            "DbError" => "rsscript_runtime::DbError".to_string(),
            "Image" => "rsscript_runtime::Image".to_string(),
            "ImageError" => "rsscript_runtime::ImageError".to_string(),
            "JsonValue" => "rsscript_runtime::JsonValue".to_string(),
            "JsonError" => "rsscript_runtime::JsonError".to_string(),
            "RowBuffer" => "rsscript_runtime::RowBuffer".to_string(),
            "Row" => "rsscript_runtime::Row".to_string(),
            "CsvError" => "rsscript_runtime::CsvError".to_string(),
            "Result" if ty.args.len() == 2 => format!(
                "Result<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "Option" if ty.args.len() == 1 => {
                format!(
                    "Option<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "List" if ty.args.len() == 1 => {
                format!(
                    "Vec<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "Deque" if ty.args.len() == 1 => format!(
                "std::collections::VecDeque<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "Map" if ty.args.len() == 2 => format!(
                "std::collections::HashMap<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "SortedMap" if ty.args.len() == 2 => format!(
                "std::collections::BTreeMap<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "PersistentMap" if ty.args.len() == 2 => format!(
                "rsscript_runtime::RssPersistentMap<{}, {}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested),
                self.lower_type_ref(&ty.args[1], ManagedPosition::Nested)
            ),
            "Set" if ty.args.len() == 1 => {
                format!(
                    "std::collections::HashSet<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "SortedSet" if ty.args.len() == 1 => {
                format!(
                    "std::collections::BTreeSet<{}>",
                    self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
                )
            }
            "ResourcePool" if ty.args.len() == 1 => format!(
                "rsscript_runtime::ResourcePool<{}>",
                self.lower_type_ref(&ty.args[0], ManagedPosition::Nested)
            ),
            "Capability" if ty.args.len() == 1 => capability_enum_name(&ty.args[0].name),
            _ => {
                let name = rust_ident(&ty.name);
                if ty.args.is_empty() {
                    name
                } else {
                    format!(
                        "{}<{}>",
                        name,
                        ty.args
                            .iter()
                            .map(|arg| self.lower_type_ref(arg, ManagedPosition::Nested))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        };

        if self.should_wrap_in_managed_handle(ty, position) {
            format!("rsscript_runtime::Managed<{lowered}>")
        } else {
            lowered
        }
    }


    pub(super) fn should_wrap_in_managed_handle(&self, ty: &TypeRef, position: ManagedPosition) -> bool {
        if !matches!(
            position,
            ManagedPosition::Param | ManagedPosition::Return | ManagedPosition::Nested
        ) {
            return false;
        }
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Class))
    }


    pub(super) fn is_class_type(&self, ty: &TypeRef) -> bool {
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Class))
    }

}
