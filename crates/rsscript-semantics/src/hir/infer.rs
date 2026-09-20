use std::collections::BTreeMap;

use rsscript_abi_model::WireType;

use crate::{ResolvedType, ResolvedTypeKind, TypeQualifiers, builtin_generic_type_params};

use super::*;

/// Converts type arguments carried by the syntax callee spelling into the
/// structural representation used by HIR inference. Callee type arguments are
/// the sole remaining textual input here; inferred local types never round-trip
/// through a display string.
pub fn resolved_type_from_source(source: &str) -> ResolvedType {
    ResolvedType::from_display(source)
}

/// Infer the type of a built-in `Option`/`Result` variant constructor call so an
/// untyped local (`let o = Some(5)`) carries a type and downstream argument checks
/// are not silently skipped. The variant's known payload position is filled from the
/// argument; the other generic position (e.g. the error type of `Ok`) is left as a
/// single-uppercase placeholder so `unresolved_generic_type` skips it rather than
/// reporting a spurious mismatch.
fn infer_enum_variant_type(
    hir: &Hir,
    variant: &str,
    args: &[rsscript_syntax::ast::CallArg],
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    let payload_type = |args: &[rsscript_syntax::ast::CallArg]| {
        args.first()
            .and_then(|arg| infer_hir_expr_type(hir, &arg.value, value_types))
    };
    match variant {
        "Some" => Some(ResolvedType::named("Option", [payload_type(args)?])),
        "Ok" => Some(ResolvedType::named(
            "Result",
            [payload_type(args)?, ResolvedType::named("E", [])],
        )),
        "Err" => Some(ResolvedType::named(
            "Result",
            [ResolvedType::named("T", []), payload_type(args)?],
        )),
        // A user-declared sum variant constructs a value of its sum type, so a `Number(value: 5)`
        // call has type `Token` — letting the normal arg/binding type checks catch misuse.
        _ => hir
            .sum_type_for_variant(variant)
            .map(|name| ResolvedType::named(name, [])),
    }
}

/// Infer the type of a builtin binary operator expression.
///
/// The operand rules are exactly `operators.rs`'s (§2.13): comparison and
/// logical operators produce `Bool`; the arithmetic and bitwise operators
/// produce their operand type, which must be one matching numeric type. There
/// is no operator overloading and no `String + String`, so anything else — an
/// operand whose type is unknown, a non-numeric operand, or a mismatched pair,
/// all of which `operators.rs` already reports as `RS0210`/`RS1001` — leaves the
/// expression untyped rather than inventing a second, derived error.
fn infer_binary_type(
    hir: &Hir,
    op: BinaryOp,
    left: &Expr,
    right: &Expr,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    match op {
        // A comparison or logical operator yields `Bool` whatever its operands
        // turn out to be; a bad operand pair is the operator check's business.
        BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::Less
        | BinaryOp::LessEqual
        | BinaryOp::Greater
        | BinaryOp::GreaterEqual
        | BinaryOp::LogicalAnd
        | BinaryOp::LogicalOr => Some(ResolvedType::named("Bool", [])),
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo
        | BinaryOp::BitAnd
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight => {
            let left = canonical_operand_root(hir, left, value_types)?;
            let right = canonical_operand_root(hir, right, value_types)?;
            if left != right || !crate::operators::is_numeric_type(&left) {
                return None;
            }
            Some(ResolvedType::named(left, []))
        }
    }
}

/// The alias-expanded root name of an operand's inferred type.
fn canonical_operand_root(hir: &Hir, expr: &Expr, value_types: &HirValueTypes) -> Option<String> {
    let ty = infer_hir_expr_type(hir, expr, value_types)?;
    let canonical = hir.canonical_type_name(&ty.to_string());
    Some(
        ResolvedType::from_display(&canonical)
            .root_name()?
            .to_owned(),
    )
}

pub fn infer_hir_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    match expr {
        // `true`, `false`, and `Unit` are literals the surface syntax spells as
        // identifiers, so they are never in `value_types`. They carry a type
        // here for the same reason they do in argument position: `let b = true`
        // must give `b` a type, or every generic construction that reads `b`
        // loses a type argument.
        Expr::Ident(name, _) => value_types
            .get(name)
            .cloned()
            .or_else(|| builtin_value_ident_type(name))
            .or_else(|| {
                hir.sum_type_for_variant(name)
                    .map(|name| ResolvedType::named(name, []))
            }),
        Expr::Binary {
            op, left, right, ..
        } => infer_binary_type(hir, *op, left, right, value_types),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            infer_hir_expr_type(hir, value, value_types)
        }
        Expr::Spawn { value, .. } => {
            infer_hir_expr_type(hir, value, value_types).map(|ty| ResolvedType::named("Task", [ty]))
        }
        Expr::Await { value, .. } => infer_hir_expr_type(hir, value, value_types)
            .and_then(|ty| task_inner_type(&ty))
            .or_else(|| infer_hir_expr_type(hir, value, value_types)),
        Expr::Try { value, .. } => infer_hir_expr_type(hir, value, value_types)
            .and_then(|ty| try_operand_payload_type(&ty)),
        // A `match` used as a value — and an `if` expression, which is a
        // `match` over `true`/`false` — has the type its arms agree on.
        Expr::Match { arms, .. } => agreeing_branch_type(
            arms.iter()
                .map(|arm| infer_closure_return_type(hir, &arm.body, value_types)),
        ),
        Expr::Call { callee, args, .. } => {
            let resolution = match callee {
                Callee::ReceiverCall {
                    receiver, method, ..
                } => {
                    if let Some(receiver_type) = infer_hir_expr_type(hir, receiver, value_types) {
                        hir.resolve_receiver_call_structured(&receiver_type, method, value_types)
                            .0
                    } else {
                        CallResolution::Unknown
                    }
                }
                _ => hir.resolve_call(callee),
            };
            match resolution {
                CallResolution::Resolved { signature, .. } => {
                    infer_signature_return_type(hir, &signature, callee, args, value_types)
                        .or_else(|| signature.return_ty.clone())
                }
                CallResolution::Ambiguous { .. } | CallResolution::Unknown => match callee {
                    Callee::Name(name) => value_types.get(name).and_then(fn_return_type),
                    Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
                },
                CallResolution::EnumVariant => {
                    infer_enum_variant_type(hir, callee_name(callee), args, value_types)
                }
            }
        }
        Expr::Field { base, name, .. } => {
            let base_type = infer_hir_expr_type(hir, base, value_types)?;
            let canonical_base_type = hir.canonical_type_name(&base_type.to_string());
            let type_info = hir.type_info(&canonical_base_type)?;
            let field = type_info.fields.get(name)?;
            Some(substituted_field_type(hir, type_info, &base_type, field))
        }
        Expr::Index { base, .. } => {
            indexed_element_type(&infer_hir_expr_type(hir, base, value_types)?)
        }
        Expr::Number(value, _) => Some(ResolvedType::named(number_literal_type_name(value), [])),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some(ResolvedType::named("String", [])),
        Expr::CharLiteral(_, _) => Some(ResolvedType::named("Char", [])),
        Expr::ObjectLiteral { .. } => Some(ResolvedType::named("JsonLiteral", [])),
        Expr::MapLiteral { .. } => Some(ResolvedType::named("MapLiteral", [])),
        Expr::ArrayLiteral { items, .. } => {
            let item_type = items
                .first()
                .and_then(|item| infer_hir_expr_type(hir, item, value_types))
                .unwrap_or_else(|| ResolvedType::named(WireType::UNRESOLVED, []));
            Some(ResolvedType::named("List", [item_type]))
        }
        // An unannotated closure's `Fn` shape is contextual: its parameter
        // types and ownership qualifier come from a declared binding or call
        // contract. `infer_arg_expr_type` deliberately has a conservative
        // noescape fallback for argument checking, but that fallback must not
        // become the type of a `let` binding. Doing so turns an ordinary
        // explicit closure into a permanently noescape value and rejects valid
        // stored/returned callbacks. HIR lowering still records an explicit
        // contextual contract on `HirExpr::Closure` where one exists.
        Expr::Closure { .. } | Expr::Unknown(_) => None,
    }
}

fn infer_signature_return_type(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    args: &[CallArg],
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    let return_type = signature.return_ty.clone()?;
    let substitutions = infer_signature_substitutions(hir, signature, callee, args, value_types)?;
    Some(return_type.substitute(&substitutions))
}

/// Return concrete generic arguments in declaration order when semantic
/// inference proved every parameter at this call site.  This is the structured
/// hand-off used by backend lowering: callers must not reconstruct generic
/// instances from callee spellings or runtime values.
pub(crate) fn infer_call_type_arguments(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    args: &[CallArg],
    value_types: &HirValueTypes,
) -> Vec<ResolvedType> {
    let Some(substitutions) =
        infer_signature_substitutions(hir, signature, callee, args, value_types)
    else {
        return Vec::new();
    };
    signature
        .type_params
        .iter()
        .map(|parameter| substitutions.get(parameter).cloned())
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

fn infer_signature_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    args: &[CallArg],
    value_types: &HirValueTypes,
) -> Option<BTreeMap<String, ResolvedType>> {
    if signature.type_params.is_empty() {
        return None;
    }
    let generic_params = signature
        .type_params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut substitutions = BTreeMap::new();
    collect_callee_type_substitutions(signature, callee, &generic_params, &mut substitutions);
    collect_namespace_type_substitutions(hir, callee, &generic_params, &mut substitutions);
    collect_receiver_type_substitutions(
        hir,
        signature,
        callee,
        value_types,
        &generic_params,
        &mut substitutions,
    );
    collect_arg_type_substitutions(
        hir,
        signature,
        args,
        value_types,
        &generic_params,
        &mut substitutions,
    );

    (!substitutions.is_empty()).then_some(substitutions)
}

fn collect_callee_type_substitutions(
    signature: &FunctionSig,
    callee: &Callee,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
) {
    let type_args = match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => type_arg_names(name),
        Callee::ReceiverCall { method, .. } => type_arg_names(method),
    };
    let Some(type_args) = type_args else {
        return;
    };
    for (param, actual) in signature.type_params.iter().zip(type_args) {
        if generic_params.contains(param.as_str()) {
            substitutions
                .entry(param.to_string())
                .or_insert_with(|| resolved_type_from_source(actual));
        }
    }
}

fn collect_namespace_type_substitutions(
    hir: &Hir,
    callee: &Callee,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
) {
    let Callee::Qualified { namespace, .. } = callee else {
        return;
    };
    let root = type_root_name(namespace);
    let Some(namespace_args) = type_arg_names(namespace) else {
        return;
    };
    let params = hir
        .type_info(root)
        .map(|type_info| {
            type_info
                .type_params
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        })
        .or_else(|| builtin_generic_type_params(root))
        .unwrap_or_default();
    for (param, actual) in params.into_iter().zip(namespace_args) {
        if generic_params.contains(param) {
            substitutions
                .entry(param.to_string())
                .or_insert_with(|| resolved_type_from_source(actual));
        }
    }
}

fn collect_receiver_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    value_types: &HirValueTypes,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
) {
    let Callee::ReceiverCall { receiver, .. } = callee else {
        return;
    };
    let Some(receiver_param) = signature.params.first() else {
        return;
    };
    let Some(actual_type) = infer_hir_expr_type(hir, receiver, value_types) else {
        return;
    };
    receiver_param
        .ty
        .clone()
        .collect_substitutions(&actual_type, generic_params, substitutions);
}

fn collect_arg_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    args: &[CallArg],
    value_types: &HirValueTypes,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
) {
    for (index, arg) in args.iter().enumerate() {
        let Some((_parameter_index, param)) = arg
            .name
            .as_deref()
            .and_then(|name| {
                signature
                    .params
                    .iter()
                    .enumerate()
                    .find(|(_, param)| param.name == name)
            })
            .or_else(|| signature.params.get(index).map(|param| (index, param)))
        else {
            continue;
        };
        // An inline closure passed to a callback contract proves exactly one
        // thing: what its body returns, matched against the contract's return
        // position. A body whose result cannot be inferred proves nothing at
        // all, so this case never falls through to the structural path below:
        // doing so would unify the callee's result parameter against the
        // closure's own unresolved marker and publish *that* as the answer.
        if param.ty.qualifiers.noescape
            && let Some(expected_return_type) = param.ty.function_return()
            && let Expr::Closure {
                params: closure_params,
                body,
                ..
            } = &arg.value
        {
            let scoped = closure_body_value_types(
                &param.ty,
                closure_params,
                value_types,
                generic_params,
                substitutions,
            );
            if let Some(actual_return_type) = infer_closure_return_type(hir, body, &scoped) {
                expected_return_type.clone().collect_substitutions(
                    &actual_return_type,
                    generic_params,
                    substitutions,
                );
            }
            continue;
        }
        let Some(actual_type) = infer_arg_expr_type(hir, &arg.value, value_types) else {
            continue;
        };
        param
            .ty
            .clone()
            .collect_substitutions(&actual_type, generic_params, substitutions);
    }
}

/// The names an inline closure's body can see while its result type is being
/// inferred.
///
/// A combinator's callback contract already names what each closure parameter
/// is — `List.map(list: List<Int>, mapper: noescape Fn(T) -> U)` proves `T =
/// Int` from the receiver argument, which is collected before this one. Without
/// binding `x` to that type, `|x| { return x * 2 }` has no inferable body and
/// `U` is left unproved, so the call's own result type (`fresh List<U>`) is
/// unusable at the binding. Substituting what is already proved into the
/// contract and binding each parameter by name is what makes the closure's
/// result a *derived* fact rather than a guess.
///
/// A parameter position the contract cannot resolve is deliberately left
/// unbound: an absent name proves nothing, which is the honest answer, while a
/// bound placeholder would be a false one.
fn closure_body_value_types(
    contract: &ResolvedType,
    closure_params: &[String],
    value_types: &HirValueTypes,
    generic_params: &HashSet<&str>,
    substitutions: &BTreeMap<String, ResolvedType>,
) -> HirValueTypes {
    let ResolvedTypeKind::Function { parameters, .. } = &contract.kind else {
        return value_types.clone();
    };
    if parameters.len() != closure_params.len() {
        return value_types.clone();
    }
    let mut scoped = value_types.clone();
    for (name, declared) in closure_params.iter().zip(parameters.iter()) {
        let bound = declared.substitute(substitutions);
        if names_unproved_type(&bound, generic_params) {
            // Nothing proved this position; leave the name unbound so the body
            // reports "not inferable" instead of inheriting a stale outer
            // binding of the same name.
            scoped.remove(name);
            continue;
        }
        scoped.insert(name.clone(), bound);
    }
    scoped
}

/// Whether a type still names something the call site has not proved: one of
/// the callee's own type parameters, or the reserved unresolved spelling.
fn names_unproved_type(ty: &ResolvedType, generic_params: &HashSet<&str>) -> bool {
    match &ty.kind {
        ResolvedTypeKind::Named { name, arguments } => {
            (arguments.is_empty()
                && (name == WireType::UNRESOLVED || generic_params.contains(name.as_str())))
                || arguments
                    .iter()
                    .any(|argument| names_unproved_type(argument, generic_params))
        }
        ResolvedTypeKind::Function {
            parameters,
            return_type,
            ..
        } => {
            parameters
                .iter()
                .any(|parameter| names_unproved_type(parameter, generic_params))
                || return_type
                    .as_deref()
                    .is_some_and(|return_type| names_unproved_type(return_type, generic_params))
        }
    }
}

pub(super) fn infer_closure_return_type(
    hir: &Hir,
    body: &Block,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    if let Some(statement) = body.statements.iter().next_back() {
        match statement {
            // `return` with no value is `Unit`; `return <expr>` is exactly as
            // typed as `<expr>` is. Defaulting an un-inferable returned
            // expression to `Unit` published a type nothing had proved, and a
            // caller unified its own result parameter against it: `List.map`
            // over a list of `Int` produced `List<Unit>`.
            Stmt::Return(stmt) => {
                return match stmt.value.as_ref() {
                    Some(value) => infer_hir_expr_type(hir, value, value_types),
                    None => Some(ResolvedType::named("Unit", [])),
                };
            }
            Stmt::Expr(value) => return infer_hir_expr_type(hir, value, value_types),
            Stmt::Let(_) | Stmt::LetElse(_) | Stmt::Assign(_) => {
                return Some(ResolvedType::named("Unit", []));
            }
            // A block whose last statement is an `if` or a `match` takes its
            // value from the branch bodies, so it is exactly as typed as they
            // agree it is.
            Stmt::If(stmt) => {
                return agreeing_branch_type(
                    std::iter::once(infer_closure_return_type(hir, &stmt.then_body, value_types))
                        .chain(stmt.else_body.as_ref().map(|else_body| {
                            infer_closure_return_type(hir, else_body, value_types)
                        })),
                );
            }
            Stmt::Match(stmt) => {
                return agreeing_branch_type(
                    stmt.arms
                        .iter()
                        .map(|arm| infer_closure_return_type(hir, &arm.body, value_types)),
                );
            }
            Stmt::With { .. }
            | Stmt::Loop { .. }
            | Stmt::For(_)
            | Stmt::TaskGroup(_)
            | Stmt::Select(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => return None,
        }
    }
    Some(ResolvedType::named("Unit", []))
}

pub(super) fn infer_arg_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    match expr {
        // A data effect names how the value is passed, not what it is.
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            infer_arg_expr_type(hir, value, value_types)
        }
        // `spawn`, `await`, and `?` each transform their operand's type, so
        // they are not pass-throughs: `f(x: await t)` is `Task<T>`'s `T`, and
        // `f(x: r?)` is `Result<T, E>`'s `T`. Reading them as the operand's own
        // type would prove a type argument that is simply wrong.
        Expr::Spawn { .. } | Expr::Await { .. } | Expr::Try { .. } => {
            infer_hir_expr_type(hir, expr, value_types)
        }
        // `true`, `false`, and `Unit` are literals that the surface syntax
        // spells as identifiers. They carry a type for the same reason the
        // scalar literals below do: a generic construction must be able to
        // unify them with a type parameter (`(1, true)` -> `B = Bool`), and a
        // parameter left unproved makes the whole call site's substitution
        // incomplete. `None` is deliberately excluded: its type is
        // `Option<?>`, which proves nothing.
        Expr::Ident(name, _) => value_types
            .get(name)
            .cloned()
            .or_else(|| builtin_value_ident_type(name)),
        Expr::Call { .. } => infer_hir_expr_type(hir, expr, value_types),
        // The contract is published even when the body's result type is not
        // proved: arity, parameter modes, and the `noescape` qualifier *are*
        // facts, and MIR lowering needs them to build the closure's ABI. Only
        // the result position records the absence, through the same reserved
        // unresolved spelling the parameters already use, which every
        // downstream consumer already projects to "no evidence".
        Expr::Closure { params, body, .. } => Some(
            infer_closure_return_type(hir, body, value_types)
                .unwrap_or_else(|| ResolvedType::named(WireType::UNRESOLVED, [])),
        )
        .map(|return_type| {
            ResolvedType::function(
                // An unannotated closure parameter has no proved type:
                // nothing in the surface syntax, the binding, or the call
                // contract names one. The reserved unresolved spelling
                // records that absence explicitly, so MIR lowering and the
                // typed executable facts report `Unknown` for the position
                // instead of inventing a nominal type.
                (0..params.len()).map(|_| ResolvedType::named(WireType::UNRESOLVED, [])),
                (0..params.len()).map(|_| None),
                Some(return_type),
                TypeQualifiers {
                    noescape: true,
                    ..TypeQualifiers::default()
                },
            )
        }),
        Expr::Match { .. } => infer_hir_expr_type(hir, expr, value_types),
        Expr::ObjectLiteral { .. } | Expr::MapLiteral { .. } | Expr::ArrayLiteral { .. } => {
            infer_hir_expr_type(hir, expr, value_types)
        }
        Expr::Field { .. } => infer_hir_expr_type(hir, expr, value_types),
        // Scalar literals carry a type so generic construction can unify it with a
        // type parameter (`Pair(item0: 1)` -> `A = Int`).
        Expr::Number(value, _) => Some(ResolvedType::named(number_literal_type_name(value), [])),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some(ResolvedType::named("String", [])),
        Expr::CharLiteral(_, _) => Some(ResolvedType::named("Char", [])),
        // An operator result is as inferable as its operands, and a generic
        // construction needs it for the same reason it needs a literal's type
        // (`(a + b, "x")` -> `A = Int`).
        Expr::Binary { .. } => infer_hir_expr_type(hir, expr, value_types),
        // An element read off a typed container proves the element type
        // (`(xs[0], xs[1])` with `xs: List<Int>` -> `A = B = Int`). This is
        // deliberately local to argument-position inference: the checker does
        // not give `let x = xs[0]` a type, and widening that is a separate
        // change with a much larger blast radius.
        Expr::Index { base, .. } => {
            indexed_element_type(&infer_arg_expr_type(hir, base, value_types)?)
        }
        Expr::Unknown(_) => None,
    }
}

/// The element type produced by `base[index]` for the container types whose
/// element position the wire format fixes. Anything else proves nothing.
fn indexed_element_type(base_type: &ResolvedType) -> Option<ResolvedType> {
    base_type
        .named_argument("List", 0)
        .or_else(|| base_type.named_argument("Map", 1))
        .cloned()
}

/// The type of `field` accessed on a value of type `base_type`, with the type's
/// generic parameters replaced by `base_type`'s concrete arguments — so `item0`
/// on `__Tuple2<Int, String>` resolves to `Int`, not the declared parameter `A`.
pub(super) fn substituted_field_type(
    _hir: &Hir,
    type_info: &TypeInfo,
    base_type: &ResolvedType,
    field: &FieldInfo,
) -> ResolvedType {
    let args = base_type.arguments();
    if args.is_empty() || type_info.type_params.is_empty() {
        return field.ty.clone();
    }
    let substitutions: BTreeMap<String, ResolvedType> = type_info
        .type_params
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    field.ty.substitute(&substitutions)
}

pub(super) fn dyn_protocol(type_name: &ResolvedType) -> Option<String> {
    type_name.named_argument("Dyn", 0).map(ToString::to_string)
}

fn fn_return_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.function_return().cloned()
}

/// The value `expr?` produces, for both types the `?` operator accepts: the ok
/// type of a `Result<T, E>` and the payload of an `Option<T>` (§6.5).
fn try_operand_payload_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name
        .named_argument("Result", 0)
        .or_else(|| type_name.named_argument("Option", 0))
        .cloned()
        .map(ResolvedType::without_fresh)
}

/// The type of a literal the surface syntax spells as an identifier.
fn builtin_value_ident_type(name: &str) -> Option<ResolvedType> {
    if !matches!(name, "true" | "false" | "Unit") {
        return None;
    }
    crate::checks::shared::builtin_value_type_name(name).map(|name| ResolvedType::named(name, []))
}

/// The single type a set of branch bodies agree on.
///
/// A `match` used as a value — an `if` expression included — has the type of
/// its arms. A branch that proves nothing is skipped, so one arm ending in a
/// `loop` does not make the whole expression untyped; but two branches proving
/// *different* types prove nothing here. That program is an `RS0209`
/// control-flow mismatch, and picking one of the two would hand lowering a
/// type argument the other branch contradicts.
fn agreeing_branch_type(
    branches: impl IntoIterator<Item = Option<ResolvedType>>,
) -> Option<ResolvedType> {
    let mut agreed: Option<ResolvedType> = None;
    for branch in branches.into_iter().flatten() {
        match &agreed {
            Some(previous) if *previous != branch => return None,
            Some(_) => {}
            None => agreed = Some(branch),
        }
    }
    agreed
}

pub(super) fn list_element_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.named_argument("List", 0).cloned()
}

pub(super) fn stream_item_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.named_argument("Stream", 0).cloned()
}

fn task_inner_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.named_argument("Task", 0).cloned()
}

pub(super) fn match_pattern_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&ResolvedType>,
) -> Option<(String, ResolvedType)> {
    match_pattern_binding_resolved_type(pattern, value_type)
}

fn match_pattern_binding_resolved_type(
    pattern: &MatchPattern,
    value_type: Option<&ResolvedType>,
) -> Option<(String, ResolvedType)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.map(|ty| (name.clone(), ty.clone()));
    }
    let MatchPattern::Variant { name, bindings, .. } = pattern else {
        return None;
    };
    // Option/Result carry a single positional payload.
    let binding = bindings.first()?;
    let value_type = value_type?;
    if name == "Some" {
        return value_type
            .named_argument("Option", 0)
            .and_then(|ty| match_pattern_binding_resolved_type(binding, Some(ty)));
    }
    match name.as_str() {
        "Ok" => value_type
            .named_argument("Result", 0)
            .and_then(|ty| match_pattern_binding_resolved_type(binding, Some(ty))),
        "Err" => value_type
            .named_argument("Result", 1)
            .and_then(|ty| match_pattern_binding_resolved_type(binding, Some(ty))),
        _ => None,
    }
}

pub(super) fn match_pattern_binding_types(
    hir: &Hir,
    pattern: &MatchPattern,
    value_type: Option<&ResolvedType>,
) -> Vec<(String, ResolvedType)> {
    let canonical_value_type = value_type.map(|ty| {
        let canonical = hir.canonical_type_name(&ty.to_string());
        resolved_type_from_source(&canonical)
    });
    let value_type = canonical_value_type.as_ref();
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type
            .map(|ty| vec![(name.clone(), ty.clone())])
            .unwrap_or_default();
    }
    if let Some(binding) = match_pattern_binding_type(pattern, value_type) {
        return vec![binding];
    }

    if let MatchPattern::Variant { name, bindings, .. } = pattern
        && !bindings.is_empty()
    {
        let Some(value_type) = value_type else {
            return Vec::new();
        };
        let root = value_type.root_name().unwrap_or_default();
        if hir
            .sum_type_for_variant(name)
            .is_some_and(|sum| sum == root)
            && let Some(field_types) = hir.sum_variant_fields.get(name)
        {
            let substitutions = binding_substitutions(hir, value_type);
            // Zip each positional sub-pattern with the variant's declared fields
            // by index.
            let mut result = Vec::new();
            for (binding, field_type) in bindings.iter().zip(field_types.iter()) {
                let field_type = field_type.ty.substitute(&substitutions);
                result.extend(match_pattern_binding_types(hir, binding, Some(&field_type)));
            }
            return result;
        }
    }

    if let MatchPattern::List {
        prefix,
        rest,
        suffix,
        ..
    } = pattern
    {
        let Some(value_type) = value_type else {
            return Vec::new();
        };
        // Element patterns bind at the list's element type `T` (`List<T>`); a
        // bound rest segment is itself a `List<T>`.
        let element_type = value_type.named_argument("List", 0).cloned();
        let mut bindings = Vec::new();
        for element in prefix.iter().chain(suffix) {
            bindings.extend(match_pattern_binding_types(
                hir,
                element,
                element_type.as_ref(),
            ));
        }
        if let Some(Some(rest_name)) = rest {
            bindings.push((rest_name.clone(), value_type.clone()));
        }
        return bindings;
    }

    let MatchPattern::Struct { name, fields, .. } = pattern else {
        return Vec::new();
    };
    let Some(value_type) = value_type else {
        return Vec::new();
    };

    let root = value_type.root_name().unwrap_or_default();
    let field_types = if hir
        .sum_type_for_variant(name)
        .is_some_and(|sum| sum == root)
    {
        hir.sum_variant_fields.get(name)
    } else {
        None
    };

    let substitutions = binding_substitutions(hir, value_type);
    if let Some(field_types) = field_types {
        return collect_struct_pattern_binding_types(hir, fields, field_types, &substitutions);
    }

    if name == root
        && let Some(type_info) = hir.type_info(root)
    {
        let field_types = type_info.fields.values().cloned().collect::<Vec<_>>();
        return collect_struct_pattern_binding_types(hir, fields, &field_types, &substitutions);
    }

    Vec::new()
}

/// Build a substitution from a generic type's declared parameters to the
/// concrete arguments in `value_type` (`Pair<Int, Int>` -> `{A: Int, B: Int}`),
/// so match-bound fields carry their resolved element types.
fn binding_substitutions(hir: &Hir, value_type: &ResolvedType) -> BTreeMap<String, ResolvedType> {
    let args = value_type.arguments();
    if args.is_empty() {
        return BTreeMap::new();
    }
    let root = value_type.root_name().unwrap_or_default();
    let params = hir
        .type_info(root)
        .map(|type_info| type_info.type_params.to_vec())
        .or_else(|| {
            builtin_generic_type_params(root)
                .map(|params| params.into_iter().map(String::from).collect())
        })
        .unwrap_or_default();
    params.into_iter().zip(args.iter().cloned()).collect()
}

fn collect_struct_pattern_binding_types(
    hir: &Hir,
    fields: &[rsscript_syntax::ast::MatchFieldPattern],
    field_types: &[FieldInfo],
    substitutions: &BTreeMap<String, ResolvedType>,
) -> Vec<(String, ResolvedType)> {
    let mut bindings = Vec::new();
    for field in fields.iter().filter(|field| !field.ignored) {
        let Some(field_type) = field_types
            .iter()
            .find(|candidate| candidate.name == field.name)
        else {
            continue;
        };
        let field_type = field_type.ty.substitute(substitutions);
        if let Some(pattern) = &field.pattern {
            bindings.extend(match_pattern_binding_types(hir, pattern, Some(&field_type)));
        } else if let Some(binding) = &field.binding {
            bindings.push((binding.clone(), field_type));
        }
    }
    bindings
}

fn classify_block_return_expr(
    hir: &Hir,
    block: &Block,
    value_types: &HirValueTypes,
) -> HirReturnProof {
    let Some(statement) = block.statements.iter().next_back() else {
        return HirReturnProof::NoValue;
    };
    match statement {
        Stmt::Return(stmt) => stmt
            .value
            .as_ref()
            .map_or(HirReturnProof::NoValue, |value| {
                classify_return_expr(hir, value, value_types)
            }),
        Stmt::Expr(value) => classify_return_expr(hir, value, value_types),
        Stmt::Let(_) | Stmt::LetElse(_) | Stmt::Assign(_) => HirReturnProof::NoValue,
        Stmt::With { .. }
        | Stmt::If { .. }
        | Stmt::Loop { .. }
        | Stmt::For(_)
        | Stmt::TaskGroup(_)
        | Stmt::Select(_)
        | Stmt::Match { .. }
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => HirReturnProof::Unknown,
    }
}

pub(super) fn classify_return_expr(
    hir: &Hir,
    expr: &Expr,
    value_types: &HirValueTypes,
) -> HirReturnProof {
    match expr {
        // `true` / `false` are boolean literals (lexed as identifiers).
        Expr::Ident(name, _) if name == "true" || name == "false" => HirReturnProof::Literal,
        // A bare payload-free sum variant (`return MUL`) names a freshly-valued
        // variant constant; it owns nothing borrowed, so it is fresh.
        Expr::Ident(name, _) if hir.sum_type_for_variant(name).is_some() => HirReturnProof::Literal,
        Expr::Ident(name, _) => HirReturnProof::Ident { name: name.clone() },
        Expr::Call { callee, args, .. } => {
            if matches!(callee_name(callee), "Err" | "None") {
                return HirReturnProof::NoValue;
            }
            if matches!(callee_name(callee), "Ok" | "Some")
                && let Some(arg) = args.first()
            {
                return classify_return_expr(hir, &arg.value, value_types);
            }
            let resolution = match callee {
                Callee::ReceiverCall {
                    receiver, method, ..
                } => infer_hir_expr_type(hir, receiver, value_types).map_or(
                    CallResolution::Unknown,
                    |receiver_type| {
                        hir.resolve_receiver_call_structured(&receiver_type, method, value_types)
                            .0
                    },
                ),
                _ => hir.resolve_call(callee),
            };
            match resolution {
                CallResolution::Resolved {
                    signature,
                    kind:
                        ResolvedCalleeKind::Constructor {
                            type_kind: HirTypeKind::Struct,
                        },
                } if signature.returns_fresh => HirReturnProof::StructConstructor,
                CallResolution::Resolved { signature, .. } if signature.returns_fresh => {
                    HirReturnProof::FreshCall
                }
                CallResolution::Resolved {
                    kind:
                        ResolvedCalleeKind::Constructor {
                            type_kind: HirTypeKind::Struct,
                        },
                    ..
                } => HirReturnProof::StructConstructor,
                // A sum/enum variant constructor (`Pair(a: 1, b: 2)`,
                // `ArgInts(values: take vals)`, `Some(x)`/`Ok(x)` wrappers) builds
                // a brand-new value, so the result is fresh; a moved-in (`take`)
                // payload transfers ownership into the fresh shell.
                CallResolution::EnumVariant => HirReturnProof::Literal,
                CallResolution::Resolved { .. }
                | CallResolution::Ambiguous { .. }
                | CallResolution::Unknown => HirReturnProof::Unknown,
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => classify_return_expr(hir, value, value_types),
        Expr::Match { arms, .. } => arms.first().map_or(HirReturnProof::Unknown, |arm| {
            classify_block_return_expr(hir, &arm.body, value_types)
        }),
        Expr::ObjectLiteral { .. } | Expr::MapLiteral { .. } | Expr::ArrayLiteral { .. } => {
            HirReturnProof::FreshCall
        }
        // String / numeric literals own nothing borrowed; returning one is fresh.
        Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _) => HirReturnProof::Literal,
        Expr::Field { .. }
        | Expr::Index { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Unknown(_) => HirReturnProof::Unknown,
    }
}
