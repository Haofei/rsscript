use std::collections::{BTreeMap, BTreeSet};

use crate::analyzer::analyze_source_with_interfaces;
use crate::diagnostic::{Diagnostic, code};
use crate::review::{ReviewMap, ReviewMapClassification};
use crate::syntax::ast::{
    ConstDecl, DataEffect, EffectDecl, Expr, FieldDecl, FunctionDecl, GenericBound, GenericParam,
    Item, Param, ProtocolDecl, ProtocolImpl, SumTypeDecl, SumVariant, TypeAliasDecl, TypeDecl,
    TypeKind, TypeRef,
};
use crate::syntax::parse_source;

use super::{PackageReviewExport, PackageReviewFileKind, PackageSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageFunctionContract {
    pub(super) name: String,
    pub(super) params: Vec<PackageParamContract>,
    pub(super) return_type: Option<String>,
    pub(super) returns_fresh: bool,
    pub(super) is_async: bool,
    pub(super) default_impl_marker: bool,
    pub(super) deprecated_reason: Option<String>,
    pub(super) effects: BTreeSet<String>,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageProtocolImplContract {
    pub(super) protocol: String,
    pub(super) type_name: String,
    pub(super) mappings: Vec<PackageProtocolImplMappingContract>,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PackageProtocolImplMappingContract {
    pub(super) method: String,
    pub(super) target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageProtocolContract {
    pub(super) name: String,
    pub(super) methods: Vec<PackageProtocolMethodContract>,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageProtocolMethodContract {
    pub(super) name: String,
    pub(super) contract: PackageFunctionContract,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageParamContract {
    pub(super) name: String,
    pub(super) effect: Option<&'static str>,
    pub(super) type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageTypeContract {
    pub(super) name: String,
    pub(super) kind: TypeKind,
    pub(super) is_opaque: bool,
    pub(super) type_params: Vec<PackageGenericContract>,
    pub(super) fields: Vec<PackageFieldContract>,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageSumTypeContract {
    pub(super) name: String,
    pub(super) type_params: Vec<PackageGenericContract>,
    pub(super) variants: Vec<PackageSumVariantContract>,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageSumVariantContract {
    pub(super) name: String,
    pub(super) fields: Vec<PackageFieldContract>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageTypeAliasContract {
    pub(super) name: String,
    pub(super) type_params: Vec<PackageGenericContract>,
    pub(super) target: String,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageConstContract {
    pub(super) name: String,
    pub(super) type_annotation: Option<String>,
    pub(super) value: Expr,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageGenericContract {
    pub(super) name: String,
    pub(super) bound: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageFieldContract {
    pub(super) name: String,
    pub(super) type_name: String,
    pub(super) is_handle: bool,
    pub(super) is_weak: bool,
}

pub(super) fn package_interface_environment_diagnostics(
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    analyze_source_with_interfaces("<package-interface-environment>", "", interfaces)
        .into_iter()
        .filter(|diagnostic| diagnostic.code == code::DUPLICATE_DECLARATION)
        .collect()
}

pub(super) fn package_interface_contract_diagnostics(
    sources: &[PackageSource],
    native_bindings: &BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    if !sources
        .iter()
        .any(|source| source.kind == PackageReviewFileKind::Source)
    {
        return Vec::new();
    }
    let source_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Source);
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Source);
    let interface_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let source_protocol_impl_contracts =
        collect_package_protocol_impl_contracts(sources, PackageReviewFileKind::Source);
    let interface_protocol_impl_contracts =
        collect_package_protocol_impl_contracts(sources, PackageReviewFileKind::Interface);
    let source_protocol_contracts =
        collect_package_protocol_contracts(sources, PackageReviewFileKind::Source);
    let interface_protocol_contracts =
        collect_package_protocol_contracts(sources, PackageReviewFileKind::Interface);
    let source_sum_type_contracts =
        collect_package_sum_type_contracts(sources, PackageReviewFileKind::Source);
    let interface_sum_type_contracts =
        collect_package_sum_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_type_alias_contracts =
        collect_package_type_alias_contracts(sources, PackageReviewFileKind::Source);
    let interface_type_alias_contracts =
        collect_package_type_alias_contracts(sources, PackageReviewFileKind::Interface);
    let source_const_contracts =
        collect_package_const_contracts(sources, PackageReviewFileKind::Source);
    let interface_const_contracts =
        collect_package_const_contracts(sources, PackageReviewFileKind::Interface);
    let mut diagnostics = Vec::new();

    for (name, interface_contract) in interface_type_contracts {
        let Some(source_contract) = source_type_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface type `{name}` has no source declaration."),
                    interface_contract.span.clone(),
                    "missing source declaration",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every interface type must be declared by package source.")
                .with_fix(
                    "implement_interface_type",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_type_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_type_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface type `{name}` does not match its source declaration."
                    ),
                    interface_contract.span.clone(),
                    "interface/source type mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_type_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_type_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source declaration so their public type contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_function_contracts {
        let Some(source_contract) = source_function_contracts.get(&name) else {
            if interface_contract.effects.contains("native") && native_bindings.contains_key(&name)
            {
                continue;
            }
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface function `{name}` has no public source implementation."
                    ),
                    interface_contract.span.clone(),
                    "missing source implementation",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every public interface function must be implemented by package source.")
                .with_fix(
                    "implement_interface_function",
                    format!("Add `pub fn {name}` with the declared signature, or remove it from the interface."),
                    "manual",
                ),
            );
            continue;
        };

        if !package_function_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface function `{name}` does not match its source implementation."
                    ),
                    interface_contract.span.clone(),
                    "interface/source signature mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_function_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_function_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source implementation so their public signatures match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_sum_type_contracts {
        let Some(source_contract) = source_sum_type_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface sum type `{name}` has no source declaration."),
                    interface_contract.span.clone(),
                    "missing source declaration",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every interface sum type must be declared by package source.")
                .with_fix(
                    "implement_interface_sum_type",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_sum_type_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_sum_type_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface sum type `{name}` does not match its source declaration."
                    ),
                    interface_contract.span.clone(),
                    "interface/source sum type mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_sum_type_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_sum_type_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source declaration so their public sum type contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_type_alias_contracts {
        let Some(source_contract) = source_type_alias_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface type alias `{name}` has no source declaration."),
                    interface_contract.span.clone(),
                    "missing source declaration",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every interface type alias must be declared by package source.")
                .with_fix(
                    "implement_interface_type_alias",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_type_alias_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_type_alias_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface type alias `{name}` does not match its source declaration."
                    ),
                    interface_contract.span.clone(),
                    "interface/source type alias mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_type_alias_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_type_alias_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source declaration so their public type alias contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_const_contracts {
        let Some(source_contract) = source_const_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface const `{name}` has no source declaration."),
                    interface_contract.span.clone(),
                    "missing source declaration",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every interface const must be declared by package source.")
                .with_fix(
                    "implement_interface_const",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_const_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_const_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface const `{name}` does not match its source declaration."),
                    interface_contract.span.clone(),
                    "interface/source const mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_const_contract_label(&interface_contract)
                ))
                .with_cause(format!("source: {}", package_const_contract_label(source_contract)))
                .with_fix(
                    "align_interface_and_source",
                    "Update the `.rssi` contract or the source declaration so their public const contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_protocol_impl_contracts {
        let Some(source_contract) = source_protocol_impl_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface protocol implementation `{name}` has no source declaration."
                    ),
                    interface_contract.span.clone(),
                    "missing source protocol implementation",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every explicit protocol implementation must be declared by package source.")
                .with_fix(
                    "implement_protocol_impl",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_protocol_impl_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_protocol_impl_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface protocol implementation `{name}` does not match its source declaration."
                    ),
                    interface_contract.span.clone(),
                    "interface/source protocol implementation mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_protocol_impl_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_protocol_impl_contract_label(source_contract)
                ))
                .with_fix(
                    "align_protocol_impl",
                    "Update the `.rssi` contract or the source `impl` mapping so their protocol implementation contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    for (name, interface_contract) in interface_protocol_contracts {
        let Some(source_contract) = source_protocol_contracts.get(&name) else {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!("package interface protocol `{name}` has no source declaration."),
                    interface_contract.span.clone(),
                    "missing source protocol declaration",
                )
                .with_cause("Package `.rssi` files are the public semantic contract; every interface protocol must be declared by package source.")
                .with_fix(
                    "implement_interface_protocol",
                    format!("Add `{}` to the package source, or remove it from the interface.", package_protocol_contract_label(&interface_contract)),
                    "manual",
                ),
            );
            continue;
        };

        if !package_protocol_contracts_match(&interface_contract, source_contract) {
            diagnostics.push(
                Diagnostic::error(
                    code::PACKAGE_INTERFACE_MISMATCH,
                    format!(
                        "package interface protocol `{name}` does not match its source declaration."
                    ),
                    interface_contract.span.clone(),
                    "interface/source protocol mismatch",
                )
                .with_cause(format!(
                    "interface: {}",
                    package_protocol_contract_label(&interface_contract)
                ))
                .with_cause(format!(
                    "source: {}",
                    package_protocol_contract_label(source_contract)
                ))
                .with_fix(
                    "align_interface_protocol",
                    "Update the `.rssi` contract or the source protocol declaration so their method contracts match exactly.",
                    "manual",
                ),
            );
        }
    }

    diagnostics
}

pub(super) fn package_type_contracts_match(
    interface: &PackageTypeContract,
    source: &PackageTypeContract,
) -> bool {
    interface.name == source.name
        && interface.kind == source.kind
        && interface.type_params == source.type_params
        && (interface.is_opaque || interface.fields == source.fields)
}

pub(super) fn package_function_contracts_match(
    interface: &PackageFunctionContract,
    source: &PackageFunctionContract,
) -> bool {
    interface.name == source.name
        && interface.params == source.params
        && interface.return_type == source.return_type
        && interface.returns_fresh == source.returns_fresh
        && interface.is_async == source.is_async
        && interface.deprecated_reason == source.deprecated_reason
        && interface.effects == source.effects
}

fn package_sum_type_contracts_match(
    interface: &PackageSumTypeContract,
    source: &PackageSumTypeContract,
) -> bool {
    interface.name == source.name
        && interface.type_params == source.type_params
        && interface.variants == source.variants
}

fn package_type_alias_contracts_match(
    interface: &PackageTypeAliasContract,
    source: &PackageTypeAliasContract,
) -> bool {
    interface.name == source.name
        && interface.type_params == source.type_params
        && interface.target == source.target
}

fn package_const_contracts_match(
    interface: &PackageConstContract,
    source: &PackageConstContract,
) -> bool {
    interface.name == source.name
        && interface.type_annotation == source.type_annotation
        && package_expr_contracts_match(&interface.value, &source.value)
}

fn package_expr_contracts_match(interface: &Expr, source: &Expr) -> bool {
    match (interface, source) {
        (Expr::Ident(left, _), Expr::Ident(right, _))
        | (Expr::Number(left, _), Expr::Number(right, _))
        | (Expr::String(left, _), Expr::String(right, _)) => left == right,
        (
            Expr::Binary {
                op: left_op,
                left: left_left,
                right: left_right,
                ..
            },
            Expr::Binary {
                op: right_op,
                left: right_left,
                right: right_right,
                ..
            },
        ) => {
            left_op == right_op
                && package_expr_contracts_match(left_left, right_left)
                && package_expr_contracts_match(left_right, right_right)
        }
        (
            Expr::Field {
                base: left_base,
                name: left_name,
                ..
            },
            Expr::Field {
                base: right_base,
                name: right_name,
                ..
            },
        ) => left_name == right_name && package_expr_contracts_match(left_base, right_base),
        (
            Expr::Index {
                base: left_base,
                index: left_index,
                ..
            },
            Expr::Index {
                base: right_base,
                index: right_index,
                ..
            },
        ) => {
            package_expr_contracts_match(left_base, right_base)
                && package_expr_contracts_match(left_index, right_index)
        }
        (
            Expr::Call {
                callee: left_callee,
                args: left_args,
                ..
            },
            Expr::Call {
                callee: right_callee,
                args: right_args,
                ..
            },
        ) => {
            left_callee == right_callee
                && left_args.len() == right_args.len()
                && left_args
                    .iter()
                    .zip(right_args.iter())
                    .all(|(left, right)| {
                        left.name == right.name
                            && left.malformed == right.malformed
                            && package_expr_contracts_match(&left.value, &right.value)
                    })
        }
        (
            Expr::Effect {
                effect: left_effect,
                value: left_value,
                ..
            },
            Expr::Effect {
                effect: right_effect,
                value: right_value,
                ..
            },
        ) => left_effect == right_effect && package_expr_contracts_match(left_value, right_value),
        (Expr::Manage { value: left, .. }, Expr::Manage { value: right, .. })
        | (Expr::Spawn { value: left, .. }, Expr::Spawn { value: right, .. })
        | (Expr::Await { value: left, .. }, Expr::Await { value: right, .. })
        | (Expr::Try { value: left, .. }, Expr::Try { value: right, .. }) => {
            package_expr_contracts_match(left, right)
        }
        (
            Expr::Closure {
                params: left_params,
                body: left_body,
                ..
            },
            Expr::Closure {
                params: right_params,
                body: right_body,
                ..
            },
        ) => left_params == right_params && left_body == right_body,
        (Expr::Unknown(_), Expr::Unknown(_)) => true,
        _ => false,
    }
}

fn package_protocol_impl_contracts_match(
    interface: &PackageProtocolImplContract,
    source: &PackageProtocolImplContract,
) -> bool {
    interface.protocol == source.protocol
        && interface.type_name == source.type_name
        && interface.mappings == source.mappings
}

fn package_protocol_contracts_match(
    interface: &PackageProtocolContract,
    source: &PackageProtocolContract,
) -> bool {
    interface.name == source.name
        && interface.methods.len() == source.methods.len()
        && interface
            .methods
            .iter()
            .zip(&source.methods)
            .all(|(interface_method, source_method)| {
                interface_method.name == source_method.name
                    && package_function_contracts_match(
                        &interface_method.contract,
                        &source_method.contract,
                    )
            })
}

/// Shared driver for the per-`Item` contract collectors.
///
/// Iterates the sources matching `kind`, parses each, and for every item runs
/// `extract`. When `extract` yields `Some((name, contract))`, the entry is
/// inserted using the same visibility rule every collector shares: interface
/// files always contribute, source files only when the declaration is public.
/// The `is_public` flag is supplied by `extract` so each variant decides how to
/// read it from its own declaration.
fn collect_package_item_contracts<T>(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
    extract: impl Fn(Item) -> Option<(String, bool, T)>,
) -> BTreeMap<String, T> {
    let mut contracts = BTreeMap::new();
    for source in sources.iter().filter(|source| source.kind == kind) {
        let program = parse_source(&source.path, &source.contents);
        for item in program.items {
            let Some((name, is_public, contract)) = extract(item) else {
                continue;
            };
            if kind == PackageReviewFileKind::Interface || is_public {
                contracts.insert(name, contract);
            }
        }
    }
    contracts
}

pub(super) fn collect_package_type_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageTypeContract> {
    collect_package_item_contracts(sources, kind, |item| {
        let Item::Type(type_decl) = item else {
            return None;
        };
        Some((
            type_decl.name.clone(),
            type_decl.is_public,
            package_type_contract(&type_decl),
        ))
    })
}

pub(super) fn collect_package_function_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageFunctionContract> {
    collect_package_item_contracts(sources, kind, |item| {
        let Item::Function(function) = item else {
            return None;
        };
        Some((
            function.name.clone(),
            function.is_public,
            package_function_contract(&function),
        ))
    })
}

pub(super) fn collect_package_sum_type_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageSumTypeContract> {
    collect_package_item_contracts(sources, kind, |item| {
        let Item::SumType(sum_type) = item else {
            return None;
        };
        Some((
            sum_type.name.clone(),
            sum_type.is_public,
            package_sum_type_contract(&sum_type),
        ))
    })
}

pub(super) fn collect_package_type_alias_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageTypeAliasContract> {
    collect_package_item_contracts(sources, kind, |item| {
        let Item::TypeAlias(alias) = item else {
            return None;
        };
        Some((
            alias.name.clone(),
            alias.is_public,
            package_type_alias_contract(&alias),
        ))
    })
}

pub(super) fn collect_package_const_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageConstContract> {
    collect_package_item_contracts(sources, kind, |item| {
        let Item::Const(const_decl) = item else {
            return None;
        };
        Some((
            const_decl.name.clone(),
            const_decl.is_public,
            package_const_contract(&const_decl),
        ))
    })
}

pub(super) fn collect_package_protocol_impl_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageProtocolImplContract> {
    let mut contracts = BTreeMap::new();
    for source in sources.iter().filter(|source| source.kind == kind) {
        let program = parse_source(&source.path, &source.contents);
        for protocol_impl in &program.protocol_impls {
            let contract = package_protocol_impl_contract(protocol_impl);
            contracts.insert(
                package_protocol_impl_contract_key(&contract.protocol, &contract.type_name),
                contract,
            );
        }
    }
    contracts
}

pub(super) fn collect_package_protocol_contracts(
    sources: &[PackageSource],
    kind: PackageReviewFileKind,
) -> BTreeMap<String, PackageProtocolContract> {
    let mut contracts = BTreeMap::new();
    for source in sources.iter().filter(|source| source.kind == kind) {
        let program = parse_source(&source.path, &source.contents);
        for protocol in &program.protocols {
            let contract = package_protocol_contract(protocol, &program.items);
            contracts.insert(contract.name.clone(), contract);
        }
    }
    contracts
}

pub(super) fn package_interface_diagnostic_exports(
    sources: &[PackageSource],
    diagnostics: &[Diagnostic],
) -> Vec<PackageReviewExport> {
    let interface_paths = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.path.as_str(), source.relative_path.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut exports = Vec::new();

    for diagnostic in diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
    {
        let Some(relative_path) = interface_paths.get(diagnostic.span.file.as_str()) else {
            continue;
        };
        let name = format!(
            "{}:{}:{}",
            relative_path, diagnostic.span.line, diagnostic.span.column
        );
        let key = (
            name.clone(),
            diagnostic.code.clone(),
            diagnostic.label.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        exports.push(PackageReviewExport {
            name,
            kind: "contract_diagnostic".to_string(),
            classification: "unknown".to_string(),
            reasons: vec![
                format!("frontend error {}", diagnostic.code),
                diagnostic.label.clone(),
            ],
            function_kind: None,
            normalized_effects: Vec::new(),
        });
    }

    exports
}

pub(super) fn package_review_exports(
    sources: &[PackageSource],
    review_map: &ReviewMap,
) -> Vec<PackageReviewExport> {
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_type_contracts;
    let type_contracts = if interface_type_contracts.is_empty() {
        source_type_contracts =
            collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_type_contracts
    } else {
        &interface_type_contracts
    };
    let interface_function_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let source_function_contracts;
    let function_contracts = if interface_function_contracts.is_empty() {
        source_function_contracts =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_function_contracts
    } else {
        &interface_function_contracts
    };
    let interface_protocol_impl_contracts =
        collect_package_protocol_impl_contracts(sources, PackageReviewFileKind::Interface);
    let source_protocol_impl_contracts;
    let protocol_impl_contracts = if interface_protocol_impl_contracts.is_empty() {
        source_protocol_impl_contracts =
            collect_package_protocol_impl_contracts(sources, PackageReviewFileKind::Source);
        &source_protocol_impl_contracts
    } else {
        &interface_protocol_impl_contracts
    };
    let interface_protocol_contracts =
        collect_package_protocol_contracts(sources, PackageReviewFileKind::Interface);
    let source_protocol_contracts;
    let protocol_contracts = if interface_protocol_contracts.is_empty() {
        source_protocol_contracts =
            collect_package_protocol_contracts(sources, PackageReviewFileKind::Source);
        &source_protocol_contracts
    } else {
        &interface_protocol_contracts
    };
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();
    let interface_sum_type_contracts =
        collect_package_sum_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_sum_type_contracts;
    let sum_type_contracts = if interface_sum_type_contracts.is_empty() {
        source_sum_type_contracts =
            collect_package_sum_type_contracts(sources, PackageReviewFileKind::Source);
        &source_sum_type_contracts
    } else {
        &interface_sum_type_contracts
    };
    let interface_type_alias_contracts =
        collect_package_type_alias_contracts(sources, PackageReviewFileKind::Interface);
    let source_type_alias_contracts;
    let type_alias_contracts = if interface_type_alias_contracts.is_empty() {
        source_type_alias_contracts =
            collect_package_type_alias_contracts(sources, PackageReviewFileKind::Source);
        &source_type_alias_contracts
    } else {
        &interface_type_alias_contracts
    };
    let interface_const_contracts =
        collect_package_const_contracts(sources, PackageReviewFileKind::Interface);
    let source_const_contracts;
    let const_contracts = if interface_const_contracts.is_empty() {
        source_const_contracts =
            collect_package_const_contracts(sources, PackageReviewFileKind::Source);
        &source_const_contracts
    } else {
        &interface_const_contracts
    };

    let mut exports = Vec::new();
    exports.extend(type_contracts.values().map(package_type_review_export));
    exports.extend(
        sum_type_contracts
            .values()
            .map(package_sum_type_review_export),
    );
    exports.extend(
        type_alias_contracts
            .values()
            .map(package_type_alias_review_export),
    );
    exports.extend(const_contracts.values().map(package_const_review_export));
    exports.extend(
        function_contracts
            .values()
            .map(|contract| package_function_review_export(contract, &resource_types, review_map)),
    );
    exports.extend(
        protocol_impl_contracts
            .values()
            .map(package_protocol_impl_review_export),
    );
    exports.extend(
        protocol_contracts
            .values()
            .map(package_protocol_review_export),
    );
    exports.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    exports
}

fn package_type_review_export(contract: &PackageTypeContract) -> PackageReviewExport {
    let mut reasons = vec![format!(
        "public {} type",
        package_type_kind_label(contract.kind)
    )];
    if contract.kind == TypeKind::Resource {
        reasons.push("resource type".to_string());
    }
    if contract.fields.iter().any(|field| field.is_handle) {
        reasons.push("handle field".to_string());
    }
    if contract.fields.iter().any(|field| field.is_weak) {
        reasons.push("weak field".to_string());
    }
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "type".to_string(),
        classification: "review_if_changed".to_string(),
        reasons,
        function_kind: None,
        normalized_effects: Vec::new(),
    }
}

fn package_sum_type_review_export(contract: &PackageSumTypeContract) -> PackageReviewExport {
    let mut reasons = vec![
        "public sum type".to_string(),
        format!("variants: {}", contract.variants.len()),
    ];
    reasons.extend(
        contract
            .variants
            .iter()
            .map(|variant| format!("variant `{}`", variant.name)),
    );
    reasons.sort();
    reasons.dedup();
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "sum_type".to_string(),
        classification: "review_if_changed".to_string(),
        reasons,
        function_kind: None,
        normalized_effects: Vec::new(),
    }
}

fn package_type_alias_review_export(contract: &PackageTypeAliasContract) -> PackageReviewExport {
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "type_alias".to_string(),
        classification: "review_if_changed".to_string(),
        reasons: vec![
            "public type alias".to_string(),
            format!("target `{}`", contract.target),
        ],
        function_kind: None,
        normalized_effects: Vec::new(),
    }
}

fn package_const_review_export(contract: &PackageConstContract) -> PackageReviewExport {
    let mut reasons = vec!["public const".to_string()];
    if let Some(type_annotation) = &contract.type_annotation {
        reasons.push(format!("type `{type_annotation}`"));
    } else {
        reasons.push("inferred type".to_string());
    }
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "const".to_string(),
        classification: "review_if_changed".to_string(),
        reasons,
        function_kind: None,
        normalized_effects: Vec::new(),
    }
}

fn package_function_review_export(
    contract: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
    review_map: &ReviewMap,
) -> PackageReviewExport {
    let mut reasons = vec!["public function".to_string()];
    if contract.is_async {
        reasons.push("async boundary".to_string());
    }
    if let Some(reason) = &contract.deprecated_reason {
        reasons.push(format!("deprecated: {reason}"));
    }
    for param in &contract.params {
        if matches!(param.effect, Some("mut" | "take")) {
            reasons.push(format!(
                "{} parameter `{}`",
                param.effect.expect("effect matched"),
                param.name
            ));
        }
        if package_type_name_has_resource_boundary(&param.type_name, resource_types) {
            reasons.push(format!("resource parameter `{}`", param.name));
        }
    }
    if contract.return_type.as_ref().is_some_and(|return_type| {
        package_type_name_has_resource_boundary(return_type, resource_types)
    }) {
        reasons.push("resource return type".to_string());
    }
    if contract.returns_fresh {
        reasons.push("returns fresh value".to_string());
    }
    for effect in &contract.effects {
        if effect.starts_with("retains(") {
            reasons.push(effect.clone());
        } else if matches!(effect.as_str(), "native" | "unsafe") {
            reasons.push(format!("{effect} boundary"));
        } else if is_guarantee_effect(effect) {
            if contract.effects.contains("native") {
                reasons.push(format!(
                    "review-only guarantee `{effect}` on native boundary"
                ));
            } else {
                reasons.push(format!("guarantee `{effect}`"));
            }
        }
    }
    let classification = if package_review_map_function_is_unknown(&contract.name, review_map) {
        reasons.push("unknown review-map region".to_string());
        "unknown"
    } else {
        "review_if_changed"
    };
    reasons.sort();
    reasons.dedup();
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "function".to_string(),
        classification: classification.to_string(),
        reasons,
        function_kind: Some(if contract.is_async {
            "async".to_string()
        } else {
            "sync".to_string()
        }),
        normalized_effects: package_function_normalized_effects(contract),
    }
}

fn package_function_normalized_effects(contract: &PackageFunctionContract) -> Vec<String> {
    let mut effects = contract.effects.iter().cloned().collect::<Vec<_>>();
    if contract.is_async && !effects.iter().any(|effect| effect == "suspends") {
        effects.push("suspends".to_string());
    }
    effects.sort();
    effects.dedup();
    effects
}

fn package_protocol_impl_review_export(
    contract: &PackageProtocolImplContract,
) -> PackageReviewExport {
    let mut reasons = vec![
        "protocol implementation".to_string(),
        format!("protocol `{}`", contract.protocol),
        format!("type `{}`", contract.type_name),
    ];
    reasons.extend(
        contract
            .mappings
            .iter()
            .map(|mapping| format!("{} = {}", mapping.method, mapping.target)),
    );
    reasons.sort();
    reasons.dedup();
    PackageReviewExport {
        name: package_protocol_impl_contract_key(&contract.protocol, &contract.type_name),
        kind: "protocol_impl".to_string(),
        classification: "review_if_changed".to_string(),
        reasons,
        function_kind: None,
        normalized_effects: Vec::new(),
    }
}

fn package_protocol_review_export(contract: &PackageProtocolContract) -> PackageReviewExport {
    let mut reasons = vec!["protocol declaration".to_string()];
    for method in &contract.methods {
        reasons.push(format!("method `{}`", method.name));
        reasons.push(format!(
            "method contract `{}`",
            package_protocol_method_contract_label(method)
        ));
    }
    reasons.sort();
    reasons.dedup();
    PackageReviewExport {
        name: contract.name.clone(),
        kind: "protocol".to_string(),
        classification: "review_if_changed".to_string(),
        reasons,
        function_kind: None,
        normalized_effects: Vec::new(),
    }
}

fn is_guarantee_effect(effect: &str) -> bool {
    matches!(effect, "no_panic" | "noalloc" | "no_block" | "pure")
}

fn package_review_map_function_is_unknown(function: &str, review_map: &ReviewMap) -> bool {
    review_map.files.iter().any(|file| {
        file.regions.iter().any(|region| {
            region.function == function && region.classification == ReviewMapClassification::Unknown
        })
    })
}

pub(super) fn package_contract_has_resource_boundary(
    contract: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
) -> bool {
    contract
        .params
        .iter()
        .any(|param| package_type_name_has_resource_boundary(&param.type_name, resource_types))
        || contract.return_type.as_ref().is_some_and(|return_type| {
            package_type_name_has_resource_boundary(return_type, resource_types)
        })
}

pub(super) fn package_type_name_has_resource_boundary(
    type_name: &str,
    resource_types: &BTreeSet<&str>,
) -> bool {
    type_name
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == '.')
        })
        .any(|part| part == "ResourcePool" || resource_types.contains(part))
}

fn package_type_contract(type_decl: &TypeDecl) -> PackageTypeContract {
    PackageTypeContract {
        name: type_decl.name.clone(),
        kind: type_decl.kind,
        is_opaque: type_decl.is_opaque,
        type_params: type_decl
            .type_params
            .iter()
            .map(package_generic_contract)
            .collect(),
        fields: type_decl
            .fields
            .iter()
            .map(package_field_contract)
            .collect(),
        span: type_decl.span.clone(),
    }
}

fn package_sum_type_contract(sum_type: &SumTypeDecl) -> PackageSumTypeContract {
    PackageSumTypeContract {
        name: sum_type.name.clone(),
        type_params: sum_type
            .type_params
            .iter()
            .map(package_generic_contract)
            .collect(),
        variants: sum_type
            .variants
            .iter()
            .map(package_sum_variant_contract)
            .collect(),
        span: sum_type.span.clone(),
    }
}

fn package_sum_variant_contract(variant: &SumVariant) -> PackageSumVariantContract {
    PackageSumVariantContract {
        name: variant.name.clone(),
        fields: variant.fields.iter().map(package_field_contract).collect(),
    }
}

fn package_type_alias_contract(alias: &TypeAliasDecl) -> PackageTypeAliasContract {
    PackageTypeAliasContract {
        name: alias.name.clone(),
        type_params: alias
            .type_params
            .iter()
            .map(package_generic_contract)
            .collect(),
        target: package_type_name(&alias.target),
        span: alias.span.clone(),
    }
}

fn package_const_contract(const_decl: &ConstDecl) -> PackageConstContract {
    PackageConstContract {
        name: const_decl.name.clone(),
        type_annotation: const_decl.type_annotation.as_ref().map(package_type_name),
        value: const_decl.value.clone(),
        span: const_decl.span.clone(),
    }
}

fn package_function_contract(function: &FunctionDecl) -> PackageFunctionContract {
    PackageFunctionContract {
        name: function.name.clone(),
        params: function.params.iter().map(package_param_contract).collect(),
        return_type: function.return_ty.as_ref().map(package_type_name),
        returns_fresh: function.returns_fresh,
        is_async: function.is_async,
        default_impl_marker: function.default_impl_marker,
        deprecated_reason: function.deprecated_reason.clone(),
        effects: function.effects.iter().map(package_effect_name).collect(),
        span: function.span.clone(),
    }
}

fn package_protocol_impl_contract(protocol_impl: &ProtocolImpl) -> PackageProtocolImplContract {
    let mut mappings = protocol_impl
        .mappings
        .iter()
        .map(|mapping| PackageProtocolImplMappingContract {
            method: mapping.method.clone(),
            target: mapping.target.clone(),
        })
        .collect::<Vec<_>>();
    mappings.sort();
    PackageProtocolImplContract {
        protocol: protocol_impl.protocol.clone(),
        type_name: protocol_impl.type_name.clone(),
        mappings,
        span: protocol_impl.span.clone(),
    }
}

fn package_generic_contract(param: &GenericParam) -> PackageGenericContract {
    PackageGenericContract {
        name: param.name.clone(),
        bound: param.bound.as_ref().map(package_generic_bound_label),
    }
}

fn package_generic_bound_label(bound: &GenericBound) -> String {
    match bound {
        GenericBound::Managed => "Managed".to_string(),
        GenericBound::Struct => "Struct".to_string(),
        GenericBound::Resource => "Resource".to_string(),
        GenericBound::Protocol(name) => name.clone(),
    }
}

fn package_field_contract(field: &FieldDecl) -> PackageFieldContract {
    PackageFieldContract {
        name: field.name.clone(),
        type_name: package_type_name(&field.ty),
        is_handle: field.is_handle,
        is_weak: field.is_weak,
    }
}

fn package_param_contract(param: &Param) -> PackageParamContract {
    PackageParamContract {
        name: param.name.clone(),
        effect: param.effect.map(package_effect_label),
        type_name: package_type_name(&param.ty),
    }
}

fn package_effect_label(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn package_effect_name(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn package_protocol_contract(protocol: &ProtocolDecl, items: &[Item]) -> PackageProtocolContract {
    let prefix = format!("{}.", protocol.name);
    let mut methods = items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => function.name.strip_prefix(&prefix).map(|method_name| {
                PackageProtocolMethodContract {
                    name: method_name.to_string(),
                    contract: package_function_contract(function),
                }
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    methods.sort_by(|left, right| {
        left.name.cmp(&right.name).then_with(|| {
            package_function_contract_label(&left.contract)
                .cmp(&package_function_contract_label(&right.contract))
        })
    });
    methods.dedup_by(|left, right| left.name == right.name && left.contract == right.contract);
    PackageProtocolContract {
        name: protocol.name.clone(),
        methods,
        span: protocol.span.clone(),
    }
}

pub(super) fn package_protocol_impl_contract_key(protocol: &str, type_name: &str) -> String {
    format!("{protocol} for {type_name}")
}

fn package_type_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(package_type_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", package_type_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(package_type_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

fn package_type_kind_label(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Resource => "resource",
    }
}

fn package_type_contract_label(contract: &PackageTypeContract) -> String {
    let type_params = if contract.type_params.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            contract
                .type_params
                .iter()
                .map(|param| match &param.bound {
                    Some(bound) => format!("{}: {bound}", param.name),
                    None => param.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let fields = if contract.fields.is_empty() {
        if contract.is_opaque {
            "<opaque>".to_string()
        } else {
            "<none>".to_string()
        }
    } else {
        contract
            .fields
            .iter()
            .map(|field| {
                if field.is_weak {
                    format!("{}: weak {}", field.name, field.type_name)
                } else if field.is_handle {
                    format!("{}: handle {}", field.name, field.type_name)
                } else {
                    format!("{}: {}", field.name, field.type_name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{}{} {}{type_params} {{ {fields} }}",
        if contract.is_opaque { "opaque " } else { "" },
        package_type_kind_label(contract.kind),
        contract.name
    )
}

fn package_sum_type_contract_label(contract: &PackageSumTypeContract) -> String {
    let type_params = package_generic_params_label(&contract.type_params);
    let variants = if contract.variants.is_empty() {
        "<none>".to_string()
    } else {
        contract
            .variants
            .iter()
            .map(|variant| {
                if variant.fields.is_empty() {
                    variant.name.clone()
                } else {
                    format!(
                        "{}({})",
                        variant.name,
                        variant
                            .fields
                            .iter()
                            .map(|field| format!("{}: {}", field.name, field.type_name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("sum {}{type_params} {{ {variants} }}", contract.name)
}

fn package_type_alias_contract_label(contract: &PackageTypeAliasContract) -> String {
    let type_params = package_generic_params_label(&contract.type_params);
    format!("type {}{type_params} = {}", contract.name, contract.target)
}

fn package_const_contract_label(contract: &PackageConstContract) -> String {
    match &contract.type_annotation {
        Some(type_annotation) => format!("const {}: {type_annotation} = <expr>", contract.name),
        None => format!("const {} = <expr>", contract.name),
    }
}

fn package_generic_params_label(params: &[PackageGenericContract]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            params
                .iter()
                .map(|param| match &param.bound {
                    Some(bound) => format!("{}: {bound}", param.name),
                    None => param.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

pub(super) fn package_function_contract_label(contract: &PackageFunctionContract) -> String {
    let params = contract
        .params
        .iter()
        .map(|param| match param.effect {
            Some(effect) => format!("{}: {} {}", param.name, effect, param.type_name),
            None => format!("{}: {}", param.name, param.type_name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let async_prefix = if contract.is_async { "async " } else { "" };
    let mut label = format!("pub {async_prefix}fn {}({params})", contract.name);
    if let Some(return_type) = &contract.return_type {
        if contract.returns_fresh {
            label.push_str(&format!(" -> fresh {return_type}"));
        } else {
            label.push_str(&format!(" -> {return_type}"));
        }
    }
    if !contract.effects.is_empty() {
        label.push_str(&format!(
            " effects({})",
            contract
                .effects
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if contract.default_impl_marker {
        label.push_str(" = _");
    }
    if let Some(reason) = &contract.deprecated_reason {
        label.push_str(&format!(
            " #deprecated(\"{}\")",
            reason.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    label
}

fn package_protocol_impl_contract_label(contract: &PackageProtocolImplContract) -> String {
    let mappings = contract
        .mappings
        .iter()
        .map(|mapping| format!("{} = {}", mapping.method, mapping.target))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "impl {} for {} {{ {} }}",
        contract.protocol, contract.type_name, mappings
    )
}

fn package_protocol_contract_label(contract: &PackageProtocolContract) -> String {
    let methods = if contract.methods.is_empty() {
        "<none>".to_string()
    } else {
        contract
            .methods
            .iter()
            .map(package_protocol_method_contract_label)
            .collect::<Vec<_>>()
            .join("; ")
    };
    format!("protocol {} {{ {} }}", contract.name, methods)
}

fn package_protocol_method_contract_label(method: &PackageProtocolMethodContract) -> String {
    let label = package_function_contract_label(&method.contract);
    label.strip_prefix("pub ").unwrap_or(&label).to_string()
}

pub(super) fn package_type_contracts_for_source(
    source: &PackageSource,
) -> BTreeMap<String, PackageTypeContract> {
    parse_source(&source.path, &source.contents)
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Type(type_decl) => {
                Some((type_decl.name.clone(), package_type_contract(&type_decl)))
            }
            Item::Function(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect()
}

pub(super) fn package_function_contracts_for_source(
    source: &PackageSource,
) -> BTreeMap<String, PackageFunctionContract> {
    parse_source(&source.path, &source.contents)
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Function(function) => {
                Some((function.name.clone(), package_function_contract(&function)))
            }
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect()
}

pub(super) fn package_added_type_contract_is_high_risk(contract: &PackageTypeContract) -> bool {
    contract.kind == TypeKind::Resource
        || contract
            .fields
            .iter()
            .any(|field| field.is_handle || field.is_weak)
}

pub(super) fn package_type_contract_boundary_changed(
    old: &PackageTypeContract,
    new: &PackageTypeContract,
) -> bool {
    old.kind != new.kind
        || old.fields.iter().zip(new.fields.iter()).any(|(old, new)| {
            old.name != new.name || old.is_handle != new.is_handle || old.is_weak != new.is_weak
        })
        || old.fields.len() != new.fields.len()
}

pub(super) fn package_added_function_contract_is_high_risk(
    contract: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
) -> bool {
    contract
        .params
        .iter()
        .any(|param| matches!(param.effect, Some("mut" | "take")))
        || contract.effects.iter().any(|effect| {
            effect.starts_with("retains(") || matches!(effect.as_str(), "native" | "unsafe")
        })
        || package_contract_has_resource_boundary(contract, resource_types)
}

pub(super) fn package_function_contract_boundary_changed(
    old: &PackageFunctionContract,
    new: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
) -> bool {
    old.is_async != new.is_async
        || function_contract_adds_mut_or_take_effect(old, new)
        || function_contract_noescape_boundary_changed(old, new)
        || function_contract_resource_boundary_changed(old, new, resource_types)
}

fn function_contract_adds_mut_or_take_effect(
    old: &PackageFunctionContract,
    new: &PackageFunctionContract,
) -> bool {
    old.params
        .iter()
        .zip(new.params.iter())
        .any(|(old, new)| old.effect != new.effect && matches!(new.effect, Some("mut" | "take")))
        || new.params.len() > old.params.len()
            && new
                .params
                .iter()
                .skip(old.params.len())
                .any(|param| matches!(param.effect, Some("mut" | "take")))
}

fn function_contract_noescape_boundary_changed(
    old: &PackageFunctionContract,
    new: &PackageFunctionContract,
) -> bool {
    old.params.iter().zip(new.params.iter()).any(|(old, new)| {
        old.type_name != new.type_name && param_noescape_boundary_changed(old, new)
    }) || old.params.len() != new.params.len()
        && old
            .params
            .iter()
            .chain(new.params.iter())
            .any(param_is_noescape_boundary)
}

fn param_noescape_boundary_changed(old: &PackageParamContract, new: &PackageParamContract) -> bool {
    param_is_noescape_boundary(old) != param_is_noescape_boundary(new)
}

fn param_is_noescape_boundary(param: &PackageParamContract) -> bool {
    param.type_name.starts_with("noescape ")
}

fn function_contract_resource_boundary_changed(
    old: &PackageFunctionContract,
    new: &PackageFunctionContract,
    resource_types: &BTreeSet<&str>,
) -> bool {
    package_contract_has_resource_boundary(old, resource_types)
        != package_contract_has_resource_boundary(new, resource_types)
}
