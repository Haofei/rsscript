//! Lower checked semantic HIR to typed CFG MIR.
//!
//! The only lowering path consumes checked HIR directly and produces
//! verifier-owned typed CFG MIR.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, WireQualifier, WireType,
};
use rsscript_mir::{
    BasicBlock, BlockId, FunctionId, MirBinaryOp, MirCallArgument, MirCallTarget,
    MirExternalImport, MirFunction, MirFunctionDebug, MirFunctionSignature, MirInstruction,
    MirInstructionSource, MirLiteral, MirModule, MirParameterMode, MirSourceLocation,
    MirTerminator, MirTypeLayout, MirVariantCaseLayout, MirVariantLayout, PlaceId, ResourceTypeId,
    TaskGroupId, TaskId, TypeId, ValueId, VerifiedMir,
};
use rsscript_semantics::{ResolvedType, ResolvedTypeKind, hir as checked};
use rsscript_text::{decode_char_token, decode_string_token, type_root_name};

// `CheckedHirLowerer` method bodies live in child modules (module-size split).
mod lowerer;
mod lowerer_calls;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirLoweringError {
    Unsupported {
        function: String,
        construct: &'static str,
    },
    Invalid(rsscript_mir::MirValidationError),
}

impl fmt::Display for MirLoweringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported {
                function,
                construct,
            } => write!(
                formatter,
                "cannot lower `{function}` to the typed MIR control-flow subset: {construct}"
            ),
            Self::Invalid(error) => error.fmt(formatter),
        }
    }
}

impl Error for MirLoweringError {}

impl From<rsscript_mir::MirValidationError> for MirLoweringError {
    fn from(value: rsscript_mir::MirValidationError) -> Self {
        Self::Invalid(value)
    }
}

/// Lower checked semantic HIR directly into verifier-owned typed CFG MIR.
///
/// All source-language resolution and ownership facts are consumed here; no
/// source-shaped executable projection exists behind this boundary.
pub fn lower_checked_hir_to_mir(hir: &checked::Hir) -> Result<VerifiedMir, MirLoweringError> {
    let mut functions = hir
        .function_bodies()
        .filter_map(|(name, body)| {
            body.block.as_ref().and_then(|block| {
                hir.resolve_function(None, name)
                    .map(|signature| (name, block, signature))
            })
        })
        .filter(|(_, _, signature)| !signature.is_external)
        .collect::<Vec<_>>();
    functions.sort_by_key(|(name, _, _)| *name);

    let mut types = TypeTable::default();
    let signatures = functions
        .iter()
        .map(|(name, _, signature)| types.checked_function_signature(name, signature))
        .collect::<Result<Vec<_>, _>>()?;
    let external_imports = checked_external_imports(hir)?;
    let async_external_binding_symbols = checked_async_external_binding_symbols(hir)?;
    let async_builtin_binding_signatures = checked_async_builtin_binding_signatures(hir)?;
    let variants = hir
        .sum_variants()
        .map(|(variant, owner, fields)| {
            (
                variant.to_owned(),
                VariantLayout {
                    owner: owner.to_owned(),
                    fields: fields.iter().map(|field| field.name.clone()).collect(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    // Construct closed-world dispatch tables before body lowering. The table
    // carries only canonical concrete receiver `TypeId`s and resolved function
    // identities; source protocol/method spellings never reach MIR.
    let function_targets = functions
        .iter()
        .enumerate()
        .map(|(index, (name, _, _))| (name.to_string(), FunctionId::new(index as u32)))
        .collect::<BTreeMap<_, _>>();
    let dynamic_protocol_methods = hir
        .call_sites()
        .iter()
        .filter_map(|call| match &call.resolution {
            checked::CallResolution::Resolved { signature, .. } => signature
                .namespace
                .as_deref()
                .filter(|namespace| {
                    !hir.protocol_method_targets(namespace, &signature.name)
                        .is_empty()
                })
                .map(|namespace| (namespace.to_owned(), signature.name.clone())),
            checked::CallResolution::Ambiguous { .. }
            | checked::CallResolution::EnumVariant
            | checked::CallResolution::Unknown => None,
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|(protocol, method)| {
            let dispatch = hir
                .protocol_method_targets(&protocol, &method)
                .into_iter()
                .map(|(type_name, target)| {
                    let receiver = types.intern(WireType::Named {
                        package: None,
                        name: type_root_name(&type_name).to_owned(),
                        arguments: Vec::new(),
                    });
                    let target = function_targets
                        .get(type_root_name(&target))
                        .copied()
                        .ok_or_else(|| MirLoweringError::Unsupported {
                            function: format!("{protocol}.{method}"),
                            construct: "dynamic protocol implementation target",
                        })?;
                    Ok((receiver, target))
                })
                .collect::<Result<Vec<_>, MirLoweringError>>()?;
            if dispatch.is_empty() {
                return Err(MirLoweringError::Unsupported {
                    function: format!("{protocol}.{method}"),
                    construct: "dynamic protocol dispatch without implementation",
                });
            }
            Ok(((protocol, method), dispatch.into_boxed_slice()))
        })
        .collect::<Result<BTreeMap<_, _>, MirLoweringError>>()?;
    let async_external_wrappers = external_imports
        .iter()
        .enumerate()
        .filter(|(_, (symbol, signature))| {
            signature.asynchronous && async_external_binding_symbols.contains(symbol.as_str())
        })
        .enumerate()
        .map(|(wrapper_index, (import_index, (symbol, signature)))| {
            let id = FunctionId::new((functions.len() + wrapper_index) as u32);
            AsyncExternalWrapper::new(
                id,
                rsscript_mir::ExternalSymbolId::new(import_index as u32),
                symbol,
                signature,
                types.wire_function_signature(signature),
            )
        })
        .collect::<Vec<_>>();
    let mut async_builtin_wrappers = Vec::with_capacity(async_builtin_binding_signatures.len());
    for (wrapper_index, (key, signature)) in async_builtin_binding_signatures.iter().enumerate() {
        let id = FunctionId::new(
            (functions.len() + async_external_wrappers.len() + wrapper_index) as u32,
        );
        async_builtin_wrappers.push(AsyncBuiltinWrapper::new(
            id,
            key,
            signature,
            types.checked_function_signature(key, signature)?,
        )?);
    }
    let targets = CallTargets {
        functions: function_targets,
        external_imports: external_imports
            .iter()
            .enumerate()
            .map(|(index, (symbol, _))| {
                (
                    symbol.as_str().to_owned(),
                    rsscript_mir::ExternalSymbolId::new(index as u32),
                )
            })
            .collect(),
        async_external_wrappers: async_external_wrappers
            .iter()
            .map(|wrapper| (wrapper.symbol.as_str().to_owned(), wrapper.id))
            .collect(),
        async_builtin_wrappers: async_builtin_wrappers
            .iter()
            .map(|wrapper| (wrapper.key.clone(), wrapper.id))
            .collect(),
        variants,
        dynamic_protocol_methods,
    };
    // Synthetic closure bodies are appended after the fixed user/async-wrapper
    // range. Allocate them from a shared registry so nested closures receive
    // stable typed function identities before their parents emit MakeClosure.
    let mut closures = ClosureRegistry::new(
        (functions.len() + async_external_wrappers.len() + async_builtin_wrappers.len()) as u32,
    );
    let mut lowered = Vec::with_capacity(functions.len());
    let mut debug = Vec::with_capacity(functions.len());
    for ((index, (name, block, signature)), mir_signature) in
        functions.iter().enumerate().zip(signatures)
    {
        let parameter_places = signature
            .params
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                (
                    parameter.name.clone(),
                    mir_signature.parameter_types()[index],
                )
            })
            .collect();
        let output = CheckedHirLowerer::new(
            CheckedHirLowererInput {
                id: FunctionId::new(index as u32),
                function_name: (*name).to_owned(),
                body: block,
                mir_signature,
                initial_places: parameter_places,
                captures: Vec::new(),
                targets: targets.clone(),
            },
            &mut types,
            &mut closures,
        )
        .lower()?;
        lowered.push(output.function);
        debug.push(output.debug);
    }
    for wrapper in async_external_wrappers {
        lowered.push(wrapper.function);
        debug.push(wrapper.debug);
    }
    for wrapper in async_builtin_wrappers {
        lowered.push(wrapper.function);
        debug.push(wrapper.debug);
    }
    for closure in closures.into_sorted() {
        lowered.push(closure.function);
        debug.push(closure.debug);
    }
    let imports = external_imports
        .into_iter()
        .enumerate()
        .map(|(index, (symbol, signature))| {
            MirExternalImport::new(
                rsscript_mir::ExternalSymbolId::new(index as u32),
                symbol,
                signature,
            )
        })
        .collect();
    let type_layouts = hir
        .types()
        .filter(|info| {
            matches!(
                info.kind,
                checked::HirTypeKind::Struct
                    | checked::HirTypeKind::Class
                    | checked::HirTypeKind::Resource
            )
        })
        .map(|info| {
            let ty = types.intern(WireType::Named {
                package: None,
                name: info.name.clone(),
                arguments: Vec::new(),
            });
            let fields = info
                .fields_ordered
                .iter()
                .map(|field| {
                    checked_type_to_wire(&field.ty, &info.name)
                        .map(|ty| (field.name.clone(), types.intern(ty)))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(MirTypeLayout::new(ty, info.name.clone(), fields))
        })
        .collect::<Result<Vec<_>, MirLoweringError>>()?;
    // `Hir` intentionally exposes user-sum declarations independently from
    // call sites. Preserve that whole table in MIR so an Artifact consumer can
    // materialize a typed final value even when a particular case is never
    // constructed in a reachable body. HIR retains source declaration order
    // separately from its name-resolution maps, which becomes the canonical
    // numeric case order at the Wire boundary.
    let mut sum_variants = BTreeMap::<String, Vec<(String, Vec<checked::FieldInfo>)>>::new();
    for (variant, owner, fields) in hir.sum_variants() {
        sum_variants
            .entry(owner.to_owned())
            .or_default()
            .push((variant.to_owned(), fields.to_vec()));
    }
    let variant_layouts = sum_variants
        .into_iter()
        .map(|(owner, variants)| {
            let ty = types.intern(WireType::Named {
                package: None,
                name: owner.clone(),
                arguments: Vec::new(),
            });
            let variants = variants
                .into_iter()
                .map(|(variant, fields)| {
                    let fields = fields
                        .into_iter()
                        .map(|field| {
                            checked_type_to_wire(&field.ty, &owner)
                                .map(|ty| (field.name, types.intern(ty)))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(MirVariantCaseLayout::new(variant, fields))
                })
                .collect::<Result<Vec<_>, MirLoweringError>>()?;
            Ok(MirVariantLayout::new(ty, owner, variants))
        })
        .collect::<Result<Vec<_>, MirLoweringError>>()?;
    Ok(MirModule::with_layouts(
        types.into_types(),
        type_layouts,
        variant_layouts,
        lowered,
        debug,
        imports,
    )?
    .into_verified()?)
}

/// External async imports need a synthetic task function only when they must
/// become a child task for `async let` or `select`. A direct
/// `await Host.call()` already suspends the current task through
/// `CallExternal`, so emitting an unused wrapper would widen the Artifact and
/// debug surface for no semantic benefit.
fn checked_async_external_binding_symbols(
    hir: &checked::Hir,
) -> Result<std::collections::BTreeSet<String>, MirLoweringError> {
    let mut symbols = std::collections::BTreeSet::new();
    for (_, body) in hir.function_bodies() {
        let Some(block) = body.block.as_ref() else {
            continue;
        };
        collect_async_external_bindings_from_block(block, &mut symbols)?;
    }
    Ok(symbols)
}

fn collect_async_external_bindings_from_block(
    block: &checked::HirBlock,
    symbols: &mut std::collections::BTreeSet<String>,
) -> Result<(), MirLoweringError> {
    for statement in &block.statements {
        collect_async_external_bindings_from_statement(statement, symbols)?;
    }
    Ok(())
}

fn collect_async_external_bindings_from_statement(
    statement: &checked::HirStmt,
    symbols: &mut std::collections::BTreeSet<String>,
) -> Result<(), MirLoweringError> {
    match statement {
        checked::HirStmt::Let {
            is_async: true,
            value: Some(checked::HirExpr::Call { resolution, .. }),
            ..
        } => {
            if let checked::CallResolution::Resolved { signature, .. } = resolution
                && signature.is_external
                && signature.is_async
                && !is_catalog_builtin(signature)
            {
                symbols.insert(checked_external_symbol(signature)?.as_str().to_owned());
            }
        }
        checked::HirStmt::With { body, .. }
        | checked::HirStmt::Loop { body, .. }
        | checked::HirStmt::For { body, .. } => {
            collect_async_external_bindings_from_block(body, symbols)?;
        }
        checked::HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_async_external_bindings_from_block(then_body, symbols)?;
            if let Some(else_body) = else_body {
                collect_async_external_bindings_from_block(else_body, symbols)?;
            }
        }
        checked::HirStmt::Match { arms, .. } => {
            for arm in arms {
                collect_async_external_bindings_from_block(&arm.body, symbols)?;
            }
        }
        checked::HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_async_external_spawn_operation(&arm.operation, symbols)?;
                collect_async_external_bindings_from_block(&arm.body, symbols)?;
            }
        }
        checked::HirStmt::Let { .. }
        | checked::HirStmt::Return { .. }
        | checked::HirStmt::Assign { .. }
        | checked::HirStmt::Break(_)
        | checked::HirStmt::Continue(_)
        | checked::HirStmt::Expr(_)
        | checked::HirStmt::Unknown(_) => {}
    }
    Ok(())
}

/// Record the external task wrapper required by one select arm. The semantic
/// checker has already established that a select operation is an async call;
/// this collector only determines whether that call needs an internal task
/// wrapper so the explicit MIR `Spawn` instruction can target it.
fn collect_async_external_spawn_operation(
    operation: &checked::HirExpr,
    symbols: &mut std::collections::BTreeSet<String>,
) -> Result<(), MirLoweringError> {
    let (operation, _) = peel_checked_select_operation(operation);
    let checked::HirExpr::Call { resolution, .. } = operation else {
        return Ok(());
    };
    let checked::CallResolution::Resolved { signature, .. } = resolution else {
        return Ok(());
    };
    if signature.is_external && signature.is_async && !is_catalog_builtin(signature) {
        symbols.insert(checked_external_symbol(signature)?.as_str().to_owned());
    }
    Ok(())
}

/// Collect async catalog builtins that need a synthetic child-task function.
/// A direct `await` executes the builtin in the current task, while `async let`
/// and `select` require a concrete `Spawn` target so task lifetime, join, and
/// cancellation remain verifier-visible.
fn checked_async_builtin_binding_signatures(
    hir: &checked::Hir,
) -> Result<BTreeMap<String, checked::FunctionSig>, MirLoweringError> {
    let mut signatures = BTreeMap::new();
    for (_, body) in hir.function_bodies() {
        let Some(block) = body.block.as_ref() else {
            continue;
        };
        collect_async_builtin_bindings_from_block(block, &mut signatures)?;
    }
    Ok(signatures)
}

fn collect_async_builtin_bindings_from_block(
    block: &checked::HirBlock,
    signatures: &mut BTreeMap<String, checked::FunctionSig>,
) -> Result<(), MirLoweringError> {
    for statement in &block.statements {
        collect_async_builtin_bindings_from_statement(statement, signatures)?;
    }
    Ok(())
}

fn collect_async_builtin_bindings_from_statement(
    statement: &checked::HirStmt,
    signatures: &mut BTreeMap<String, checked::FunctionSig>,
) -> Result<(), MirLoweringError> {
    match statement {
        checked::HirStmt::Let {
            is_async: true,
            value: Some(checked::HirExpr::Call { resolution, .. }),
            ..
        } => collect_async_builtin_signature(resolution, signatures),
        checked::HirStmt::With { body, .. }
        | checked::HirStmt::Loop { body, .. }
        | checked::HirStmt::For { body, .. } => {
            collect_async_builtin_bindings_from_block(body, signatures)
        }
        checked::HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_async_builtin_bindings_from_block(then_body, signatures)?;
            if let Some(else_body) = else_body {
                collect_async_builtin_bindings_from_block(else_body, signatures)?;
            }
            Ok(())
        }
        checked::HirStmt::Match { arms, .. } => {
            for arm in arms {
                collect_async_builtin_bindings_from_block(&arm.body, signatures)?;
            }
            Ok(())
        }
        checked::HirStmt::Select { arms, .. } => {
            for arm in arms {
                let (operation, _) = peel_checked_select_operation(&arm.operation);
                if let checked::HirExpr::Call { resolution, .. } = operation {
                    collect_async_builtin_signature(resolution, signatures)?;
                }
                collect_async_builtin_bindings_from_block(&arm.body, signatures)?;
            }
            Ok(())
        }
        checked::HirStmt::Let { .. }
        | checked::HirStmt::Return { .. }
        | checked::HirStmt::Assign { .. }
        | checked::HirStmt::Break(_)
        | checked::HirStmt::Continue(_)
        | checked::HirStmt::Expr(_)
        | checked::HirStmt::Unknown(_) => Ok(()),
    }
}

fn collect_async_builtin_signature(
    resolution: &checked::CallResolution,
    signatures: &mut BTreeMap<String, checked::FunctionSig>,
) -> Result<(), MirLoweringError> {
    let checked::CallResolution::Resolved { signature, .. } = resolution else {
        return Ok(());
    };
    if !signature.is_async || !is_catalog_builtin(signature) {
        return Ok(());
    }
    let namespace =
        signature
            .namespace
            .as_deref()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: signature.name.clone(),
                construct: "async builtin checked HIR call without namespace",
            })?;
    signatures
        .entry(format!("{namespace}.{}", signature.name))
        .or_insert_with(|| signature.as_ref().clone());
    Ok(())
}

fn checked_external_imports(
    hir: &checked::Hir,
) -> Result<Vec<(ExternalSymbol, FunctionSignature)>, MirLoweringError> {
    let mut imports = BTreeMap::new();
    for call in hir.call_sites() {
        let checked::CallResolution::Resolved { signature, .. } = &call.resolution else {
            continue;
        };
        if !signature.is_external || is_catalog_builtin(signature) {
            continue;
        }
        let symbol = checked_external_symbol(signature)?;
        imports
            .entry(symbol.as_str().to_owned())
            .or_insert((symbol, checked_external_signature(hir, signature)?));
    }
    Ok(imports.into_values().collect())
}

fn checked_external_symbol(
    signature: &checked::FunctionSig,
) -> Result<ExternalSymbol, MirLoweringError> {
    let Some(namespace) = signature.namespace.as_deref() else {
        return Err(MirLoweringError::Unsupported {
            function: signature.name.clone(),
            construct: "external checked HIR call without namespace",
        });
    };
    ExternalSymbol::new(format!("{namespace}.{}", signature.name)).map_err(|_| {
        MirLoweringError::Unsupported {
            function: signature.name.clone(),
            construct: "invalid external checked HIR symbol",
        }
    })
}

fn checked_external_signature(
    hir: &checked::Hir,
    signature: &checked::FunctionSig,
) -> Result<FunctionSignature, MirLoweringError> {
    Ok(FunctionSignature {
        parameters: signature
            .params
            .iter()
            .map(|parameter| {
                Ok(ParameterSignature {
                    name: parameter.name.clone(),
                    effect: match parameter.effect.unwrap_or(checked::ParamEffect::Read) {
                        checked::ParamEffect::Read => DataEffect::Read,
                        checked::ParamEffect::Mut => DataEffect::Mut,
                        checked::ParamEffect::Take => DataEffect::Take,
                    },
                    ty: checked_external_type_to_wire(hir, signature, &parameter.ty)?,
                    retained: signature.retained_params.contains(&parameter.name),
                })
            })
            .collect::<Result<Vec<_>, MirLoweringError>>()?,
        result: signature
            .return_ty
            .as_ref()
            .map(|ty| checked_external_type_to_wire(hir, signature, ty))
            .transpose()?
            .unwrap_or(WireType::Unit),
        asynchronous: signature.is_async,
    })
}

/// Select operations are syntactically an `await` boundary and may be wrapped
/// in `?` or a data-effect annotation. MIR spawns the underlying resolved call
/// and reapplies `?` to the winning result after `Select` has closed all arm
/// tasks, matching the legacy scheduler ordering without retaining source
/// syntax in the backend representation.
fn peel_checked_select_operation(operation: &checked::HirExpr) -> (&checked::HirExpr, bool) {
    let mut current = operation;
    let mut has_try = false;
    loop {
        match current {
            checked::HirExpr::Try { value, .. } => {
                has_try = true;
                current = value;
            }
            checked::HirExpr::Await { value, .. } | checked::HirExpr::Effect { value, .. } => {
                current = value;
            }
            other => return (other, has_try),
        }
    }
}

type DynamicProtocolMethods = BTreeMap<(String, String), Box<[(TypeId, FunctionId)]>>;

#[derive(Clone)]
struct CallTargets {
    functions: BTreeMap<String, FunctionId>,
    external_imports: BTreeMap<String, rsscript_mir::ExternalSymbolId>,
    async_external_wrappers: BTreeMap<String, FunctionId>,
    async_builtin_wrappers: BTreeMap<String, FunctionId>,
    variants: BTreeMap<String, VariantLayout>,
    dynamic_protocol_methods: DynamicProtocolMethods,
}

/// Synthetic async functions let `async let value = Host.call()` and
/// `select { value = await Host.call() => ... }` retain the structured task
/// model without adding a second Provider-specific spawn instruction to MIR.
/// The wrapper contains only a resolved external call and is generated from
/// the same checked signature as the import table.
struct AsyncExternalWrapper {
    id: FunctionId,
    symbol: ExternalSymbol,
    function: MirFunction,
    debug: MirFunctionDebug,
}

impl AsyncExternalWrapper {
    fn new(
        id: FunctionId,
        external: rsscript_mir::ExternalSymbolId,
        symbol: &ExternalSymbol,
        signature: &FunctionSignature,
        mir_signature: MirFunctionSignature,
    ) -> Self {
        let arguments = signature
            .parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| match parameter.effect {
                DataEffect::Read => MirCallArgument::BorrowRead(PlaceId::new(index as u32)),
                DataEffect::Mut => MirCallArgument::BorrowMut(PlaceId::new(index as u32)),
                DataEffect::Take => MirCallArgument::Take(PlaceId::new(index as u32)),
            })
            .collect();
        let result = ValueId::new(0);
        let function = MirFunction::new(
            id,
            mir_signature,
            signature.parameters.len() as u32,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: result,
                    target: MirCallTarget::External(external),
                    arguments,
                }],
                MirTerminator::Return(Some(result)),
            )],
        );
        let debug = MirFunctionDebug::new(
            format!("__rss_async_external_{}", symbol.as_str().replace('.', "_")),
            signature
                .parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect(),
        );
        Self {
            id,
            symbol: symbol.clone(),
            function,
            debug,
        }
    }
}

/// Synthetic async builtin functions keep `async let` and `select` on the
/// same structured-task model as internal calls without falsely creating a
/// Provider import for a VM-owned operation. The wrapper is assembled only
/// from checked signature facts and a catalog `BuiltinId`; it carries no
/// source callee spelling into executable MIR.
struct AsyncBuiltinWrapper {
    id: FunctionId,
    key: String,
    function: MirFunction,
    debug: MirFunctionDebug,
}

impl AsyncBuiltinWrapper {
    fn new(
        id: FunctionId,
        key: &str,
        signature: &checked::FunctionSig,
        mir_signature: MirFunctionSignature,
    ) -> Result<Self, MirLoweringError> {
        let namespace =
            signature
                .namespace
                .as_deref()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: signature.name.clone(),
                    construct: "async builtin checked HIR call without namespace",
                })?;
        let builtin = rsscript_mir::builtin_id(namespace, &signature.name).ok_or_else(|| {
            MirLoweringError::Unsupported {
                function: signature.name.clone(),
                construct: "async checked HIR call without catalog builtin identity",
            }
        })?;
        let arguments = signature
            .params
            .iter()
            .enumerate()
            .map(|(index, parameter)| match parameter.effect {
                Some(checked::ParamEffect::Read) | None => {
                    MirCallArgument::BorrowRead(PlaceId::new(index as u32))
                }
                Some(checked::ParamEffect::Mut) => {
                    MirCallArgument::BorrowMut(PlaceId::new(index as u32))
                }
                Some(checked::ParamEffect::Take) => {
                    MirCallArgument::Take(PlaceId::new(index as u32))
                }
            })
            .collect();
        let result = ValueId::new(0);
        let function = MirFunction::new(
            id,
            mir_signature,
            signature.params.len() as u32,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: result,
                    target: MirCallTarget::Builtin {
                        id: builtin,
                        parameter_modes: signature
                            .params
                            .iter()
                            .map(|parameter| match parameter.effect {
                                Some(checked::ParamEffect::Read) | None => MirParameterMode::Read,
                                Some(checked::ParamEffect::Mut) => MirParameterMode::Mut,
                                Some(checked::ParamEffect::Take) => MirParameterMode::Take,
                            })
                            .collect(),
                        type_arguments: Box::new([]),
                    },
                    arguments,
                }],
                MirTerminator::Return(Some(result)),
            )],
        );
        let debug = MirFunctionDebug::new(
            format!("__rss_async_builtin_{}", key.replace('.', "_")),
            signature
                .params
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect(),
        );
        Ok(Self {
            id,
            key: key.to_owned(),
            function,
            debug,
        })
    }
}

#[derive(Debug, Clone)]
struct VariantLayout {
    owner: String,
    fields: Vec<String>,
}

/// Bindings made available after a checked match edge. User sum variants use
/// their semantic layout; `Result` is a language primitive and therefore has
/// a dedicated typed payload projection instead of a synthetic source name.
enum MatchBindings {
    Variant(VariantLayout, Vec<rsscript_syntax::ast::MatchPattern>),
    Result {
        ok: bool,
        binding: rsscript_syntax::ast::MatchPattern,
    },
    Option {
        some: bool,
        binding: Option<rsscript_syntax::ast::MatchPattern>,
    },
}

#[derive(Default)]
struct TypeTable {
    ids: BTreeMap<WireType, TypeId>,
    types: Vec<WireType>,
}

impl TypeTable {
    fn intern(&mut self, ty: WireType) -> TypeId {
        if let Some(id) = self.ids.get(&ty) {
            return *id;
        }
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(ty.clone());
        self.ids.insert(ty, id);
        id
    }

    fn checked_function_signature(
        &mut self,
        function_name: &str,
        signature: &checked::FunctionSig,
    ) -> Result<MirFunctionSignature, MirLoweringError> {
        Ok(MirFunctionSignature::with_modes(
            signature
                .params
                .iter()
                .map(|parameter| {
                    checked_type_to_wire(&parameter.ty, function_name).map(|ty| self.intern(ty))
                })
                .collect::<Result<Vec<_>, _>>()?,
            checked_parameter_modes(signature),
            self.intern(
                signature
                    .return_ty
                    .as_ref()
                    .map(|ty| checked_type_to_wire(ty, function_name))
                    .transpose()?
                    .unwrap_or(WireType::Unit),
            ),
            signature.is_async,
        ))
    }

    fn wire_function_signature(&mut self, signature: &FunctionSignature) -> MirFunctionSignature {
        MirFunctionSignature::with_modes(
            signature
                .parameters
                .iter()
                .map(|parameter| self.intern(parameter.ty.clone()))
                .collect(),
            signature
                .parameters
                .iter()
                .map(|parameter| match parameter.effect {
                    DataEffect::Read => MirParameterMode::Read,
                    DataEffect::Mut => MirParameterMode::Mut,
                    DataEffect::Take => MirParameterMode::Take,
                })
                .collect(),
            self.intern(signature.result.clone()),
            signature.asynchronous,
        )
    }

    fn into_types(self) -> Vec<WireType> {
        self.types
    }
}

/// Project checked parameter effects into the ownership modes carried by MIR.
/// Kept beside the type table so every call target, including closed-world
/// protocol dispatch, uses the same canonical ABI interpretation.
fn checked_parameter_modes(signature: &checked::FunctionSig) -> Vec<MirParameterMode> {
    signature
        .params
        .iter()
        .map(
            |parameter| match parameter.effect.unwrap_or(checked::ParamEffect::Read) {
                checked::ParamEffect::Read => MirParameterMode::Read,
                checked::ParamEffect::Mut => MirParameterMode::Mut,
                checked::ParamEffect::Take => MirParameterMode::Take,
            },
        )
        .collect()
}

/// Lower a checked structural function value to the typed callable ABI used by
/// `MakeClosure` and `CallClosure`. This intentionally unwraps only the
/// function node itself; each parameter and result still passes through the
/// same structural wire conversion as ordinary MIR function signatures.
fn checked_closure_signature(
    types: &mut TypeTable,
    ty: &rsscript_semantics::ResolvedType,
    function_name: &str,
) -> Result<MirFunctionSignature, MirLoweringError> {
    let ResolvedTypeKind::Function {
        parameters,
        parameter_effects,
        return_type,
    } = &ty.kind
    else {
        return Err(MirLoweringError::Unsupported {
            function: function_name.to_owned(),
            construct: "non-function checked HIR closure contract",
        });
    };
    if parameters.len() != parameter_effects.len() {
        return Err(MirLoweringError::Unsupported {
            function: function_name.to_owned(),
            construct: "malformed checked HIR closure parameter effects",
        });
    }
    let parameter_types = parameters
        .iter()
        .map(|parameter| checked_type_to_wire(parameter, function_name).map(|ty| types.intern(ty)))
        .collect::<Result<Vec<_>, _>>()?;
    let parameter_modes = parameter_effects
        .iter()
        .map(
            |effect| match effect.unwrap_or(rsscript_semantics::ResolvedParamEffect::Read) {
                rsscript_semantics::ResolvedParamEffect::Read => MirParameterMode::Read,
                rsscript_semantics::ResolvedParamEffect::Mut => MirParameterMode::Mut,
                rsscript_semantics::ResolvedParamEffect::Take => MirParameterMode::Take,
            },
        )
        .collect();
    let result = return_type
        .as_deref()
        .map(|result| checked_type_to_wire(result, function_name))
        .transpose()?
        .unwrap_or(WireType::Unit);
    Ok(MirFunctionSignature::with_modes(
        parameter_types,
        parameter_modes,
        types.intern(result),
        false,
    ))
}

struct LoweredFunction {
    function: MirFunction,
    debug: MirFunctionDebug,
}

/// Callable ABI retained for a local first-class closure value. The target is
/// intentionally absent: a `CallClosure` dispatches through the runtime value,
/// while the parameter contract stays verifier-visible and source-free.
#[derive(Clone)]
struct ClosureAbi {
    parameter_types: Box<[TypeId]>,
    parameter_modes: Box<[MirParameterMode]>,
}

impl From<&MirFunctionSignature> for ClosureAbi {
    fn from(signature: &MirFunctionSignature) -> Self {
        Self {
            parameter_types: signature.parameter_types().to_vec().into_boxed_slice(),
            parameter_modes: signature.parameter_modes().to_vec().into_boxed_slice(),
        }
    }
}

/// Owns synthetic closure functions while a checked-HIR module is lowered.
/// The ordinary function and async-wrapper ranges are fixed before body
/// lowering, so this registry is the one allocator for all nested closure
/// identities. Sorting by the typed ID before module construction keeps the
/// verifier's index-based function table deterministic.
struct ClosureRegistry {
    next_id: u32,
    functions: Vec<LoweredFunction>,
}

impl ClosureRegistry {
    fn new(next_id: u32) -> Self {
        Self {
            next_id,
            functions: Vec::new(),
        }
    }

    fn allocate(&mut self) -> FunctionId {
        let id = FunctionId::new(self.next_id);
        self.next_id += 1;
        id
    }

    fn push(&mut self, function: LoweredFunction) {
        self.functions.push(function);
    }

    fn into_sorted(mut self) -> Vec<LoweredFunction> {
        self.functions
            .sort_by_key(|function| function.function.id().index());
        self.functions
    }
}

/// Checked-HIR lowering owns the sole frontend-to-MIR transition. It consumes
/// semantic facts directly and never reconstructs a source-shaped backend IR.
struct CheckedHirLowerer<'source, 'types, 'closures> {
    id: FunctionId,
    function_name: String,
    body: &'source checked::HirBlock,
    mir_signature: MirFunctionSignature,
    captures: Vec<rsscript_mir::MirClosureCapture>,
    targets: CallTargets,
    types: &'types mut TypeTable,
    closure_registry: &'closures mut ClosureRegistry,
    blocks: Vec<BlockDraft>,
    current: BlockId,
    places: HashMap<String, PlaceId>,
    place_types: HashMap<String, TypeId>,
    closure_abis: HashMap<String, ClosureAbi>,
    place_names: Vec<String>,
    instruction_sources: Vec<MirInstructionSource>,
    next_value: u32,
    tasks: HashMap<String, TaskId>,
    next_task: u32,
    loops: Vec<LoopTargets>,
    resource_scopes: Vec<PlaceId>,
}

struct CheckedHirLowererInput<'source> {
    id: FunctionId,
    function_name: String,
    body: &'source checked::HirBlock,
    mir_signature: MirFunctionSignature,
    initial_places: Vec<(String, TypeId)>,
    captures: Vec<rsscript_mir::MirClosureCapture>,
    targets: CallTargets,
}

fn is_catalog_builtin(signature: &checked::FunctionSig) -> bool {
    signature
        .namespace
        .as_deref()
        .is_some_and(|namespace| rsscript_mir::builtin_id(namespace, &signature.name).is_some())
}

fn is_json_decode_builtin(signature: &checked::FunctionSig) -> bool {
    matches!(
        (signature.namespace.as_deref(), signature.name.as_str()),
        (Some("Json"), "decode" | "decode_text")
    )
}

fn callee_type_arguments(callee: &rsscript_syntax::ast::Callee) -> Option<Vec<&str>> {
    let spelling = match callee {
        rsscript_syntax::ast::Callee::Name(name) => name.as_str(),
        rsscript_syntax::ast::Callee::Qualified { name, .. } => name.as_str(),
        rsscript_syntax::ast::Callee::ReceiverCall { .. } => return None,
    };
    rsscript_text::type_arg_names(spelling)
}

struct BlockDraft {
    instructions: Vec<MirInstruction>,
    terminator: Option<MirTerminator>,
}

impl BlockDraft {
    fn new() -> Self {
        Self {
            instructions: Vec::new(),
            terminator: None,
        }
    }
}

struct LoopTargets {
    continue_target: BlockId,
    break_target: BlockId,
    cleanup_depth: usize,
}
/// Convert semantic type facts into the provider-neutral wire representation
/// without round-tripping through a rendered type string. This keeps source
/// spelling and formatting changes out of MIR identity. Function values are
/// lowered through the dedicated structural closure ABI above; arbitrary
/// function values in ordinary wire positions still fail closed rather than
/// silently becoming synthetic named types.
fn checked_type_to_wire(
    ty: &rsscript_semantics::ResolvedType,
    function_name: &str,
) -> Result<WireType, MirLoweringError> {
    let base = match &ty.kind {
        ResolvedTypeKind::Function { .. } => {
            return Err(MirLoweringError::Unsupported {
                function: function_name.to_owned(),
                construct: "function type in direct MIR signature",
            });
        }
        ResolvedTypeKind::Named { name, arguments } => {
            let arguments = arguments
                .iter()
                .map(|argument| checked_type_to_wire(argument, function_name))
                .collect::<Result<Vec<_>, _>>()?;
            match (name.as_str(), arguments.as_slice()) {
                ("Unit", []) => WireType::Unit,
                ("Bool", []) => WireType::Bool,
                ("Int", []) => WireType::Int {
                    bits: 64,
                    signed: true,
                },
                ("Float", []) => WireType::Float { bits: 64 },
                ("String", []) => WireType::String,
                ("Char", []) => WireType::Char,
                ("Bytes", []) => WireType::Bytes,
                ("List", [element]) => WireType::List {
                    element: Box::new(element.clone()),
                },
                ("Map", [key, value]) => WireType::Map {
                    key: Box::new(key.clone()),
                    value: Box::new(value.clone()),
                },
                ("Option", [value]) => WireType::Option {
                    value: Box::new(value.clone()),
                },
                ("Result", [ok, error]) => WireType::Result {
                    ok: Box::new(ok.clone()),
                    error: Box::new(error.clone()),
                },
                _ => {
                    let (package, name) = name.rsplit_once('.').map_or_else(
                        || (None, name.clone()),
                        |(package, name)| (Some(package.to_owned()), name.to_owned()),
                    );
                    WireType::Named {
                        package,
                        name,
                        arguments,
                    }
                }
            }
        }
    };
    Ok(apply_checked_qualifiers(ty, base))
}

/// Convert an external call contract using both the resolved type and the HIR
/// declaration kind.  `ResolvedType` intentionally stores a canonical type
/// identity, but it does not duplicate whether that identity was declared as
/// an opaque resource.  The Artifact ABI must retain that distinction: a
/// resource is a generation-safe Provider handle, not an ordinary named
/// record.  This keeps compiler imports byte-for-byte compatible with the
/// descriptor generated from the same `.rssi` interface.
fn checked_external_type_to_wire(
    hir: &checked::Hir,
    signature: &checked::FunctionSig,
    ty: &rsscript_semantics::ResolvedType,
) -> Result<WireType, MirLoweringError> {
    let ResolvedTypeKind::Named { name, arguments } = &ty.kind else {
        return checked_type_to_wire(ty, &signature.name);
    };
    if !arguments.is_empty() || hir.type_kind(name) != Some(checked::HirTypeKind::Resource) {
        return checked_type_to_wire(ty, &signature.name);
    }
    let Some(namespace) = signature.namespace.as_deref() else {
        return Err(MirLoweringError::Unsupported {
            function: signature.name.clone(),
            construct: "external resource type without namespace",
        });
    };
    // Module isolation represents `host.session.Session` internally as
    // `host_session__Session`.  The external function namespace remains the
    // authoritative ABI module spelling, so use it to recover the resource's
    // public type name rather than publishing the private mangled identity.
    let local_name = name
        .rsplit_once("__")
        .map_or(name.as_str(), |(_, tail)| tail);
    Ok(apply_checked_qualifiers(
        ty,
        WireType::Resource {
            name: format!("{namespace}.{local_name}"),
        },
    ))
}

fn apply_checked_qualifiers(ty: &rsscript_semantics::ResolvedType, base: WireType) -> WireType {
    let base = if ty.qualifiers.owned && !ty.qualifiers.noescape {
        WireType::Qualified {
            qualifier: WireQualifier::Owned,
            value: Box::new(base),
        }
    } else {
        base
    };
    let base = if ty.qualifiers.noescape {
        WireType::Qualified {
            qualifier: WireQualifier::NoEscape,
            value: Box::new(base),
        }
    } else {
        base
    };
    if ty.qualifiers.fresh {
        WireType::Qualified {
            qualifier: WireQualifier::Fresh,
            value: Box::new(base),
        }
    } else {
        base
    }
}

/// Resource declarations are represented as semantic named types until an
/// Artifact-wide type-layout table exists. The direct lowering boundary turns
/// only those resolved names into the MIR resource identity; arbitrary
/// aggregates remain invalid resource inputs rather than acquiring a synthetic
/// string identity.
fn resource_type_name_from_wire(wire: &WireType) -> Option<String> {
    match wire {
        WireType::Resource { name } | WireType::Handle { name } => Some(name.clone()),
        WireType::Named { package, name, .. } => Some(
            package
                .as_ref()
                .map(|package| format!("{package}.{name}"))
                .unwrap_or_else(|| name.clone()),
        ),
        WireType::Qualified { value, .. } => resource_type_name_from_wire(value),
        WireType::Unit
        | WireType::Bool
        | WireType::Int { .. }
        | WireType::Float { .. }
        | WireType::String
        | WireType::Char
        | WireType::Bytes
        | WireType::List { .. }
        | WireType::Map { .. }
        | WireType::Option { .. }
        | WireType::Result { .. }
        | WireType::Tuple { .. } => None,
    }
}

fn checked_binary_op(op: rsscript_syntax::ast::BinaryOp) -> MirBinaryOp {
    match op {
        rsscript_syntax::ast::BinaryOp::Add => MirBinaryOp::Add,
        rsscript_syntax::ast::BinaryOp::Subtract => MirBinaryOp::Subtract,
        rsscript_syntax::ast::BinaryOp::Multiply => MirBinaryOp::Multiply,
        rsscript_syntax::ast::BinaryOp::Divide => MirBinaryOp::Divide,
        rsscript_syntax::ast::BinaryOp::Modulo => MirBinaryOp::Modulo,
        rsscript_syntax::ast::BinaryOp::BitAnd => MirBinaryOp::BitAnd,
        rsscript_syntax::ast::BinaryOp::BitOr => MirBinaryOp::BitOr,
        rsscript_syntax::ast::BinaryOp::BitXor => MirBinaryOp::BitXor,
        rsscript_syntax::ast::BinaryOp::ShiftLeft => MirBinaryOp::ShiftLeft,
        rsscript_syntax::ast::BinaryOp::ShiftRight => MirBinaryOp::ShiftRight,
        rsscript_syntax::ast::BinaryOp::Equal => MirBinaryOp::Equal,
        rsscript_syntax::ast::BinaryOp::NotEqual => MirBinaryOp::NotEqual,
        rsscript_syntax::ast::BinaryOp::Less => MirBinaryOp::Less,
        rsscript_syntax::ast::BinaryOp::LessEqual => MirBinaryOp::LessEqual,
        rsscript_syntax::ast::BinaryOp::Greater => MirBinaryOp::Greater,
        rsscript_syntax::ast::BinaryOp::GreaterEqual => MirBinaryOp::GreaterEqual,
        rsscript_syntax::ast::BinaryOp::LogicalAnd => MirBinaryOp::LogicalAnd,
        rsscript_syntax::ast::BinaryOp::LogicalOr => MirBinaryOp::LogicalOr,
    }
}

/// Booleans and `Unit` are represented as identifiers by the source-shaped
/// HIR. They are language literals, not local places, even when semantic call
/// binding wraps them in a `read` effect. Keeping that distinction here avoids
/// projecting a literal argument into an invalid `BorrowRead` operation.
fn is_checked_literal_ident(expression: &checked::HirExpr) -> bool {
    matches!(
        expression,
        checked::HirExpr::Ident { name, .. } if matches!(name.as_str(), "Unit" | "true" | "false")
    )
}

fn match_literal(
    literal: &rsscript_syntax::ast::MatchLiteral,
    function_name: &str,
) -> Result<MirLiteral, MirLoweringError> {
    match literal {
        rsscript_syntax::ast::MatchLiteral::Int(value) => value
            .parse::<i64>()
            .map(MirLiteral::Int)
            .or_else(|_| value.parse::<f64>().map(MirLiteral::Float))
            .map_err(|_| MirLoweringError::Unsupported {
                function: function_name.to_owned(),
                construct: "non-numeric checked HIR match literal",
            }),
        rsscript_syntax::ast::MatchLiteral::String(value) => {
            Ok(MirLiteral::String(decode_string_token(value)))
        }
        rsscript_syntax::ast::MatchLiteral::Char(value) => {
            Ok(MirLiteral::Char(decode_char_token(value)))
        }
        rsscript_syntax::ast::MatchLiteral::Bool(value) => Ok(MirLiteral::Bool(*value)),
    }
}

fn result_variant_tag(name: &str) -> Option<bool> {
    match name {
        "Ok" => Some(true),
        "Err" => Some(false),
        _ => None,
    }
}

fn option_variant_tag(name: &str) -> Option<bool> {
    match name {
        "Some" => Some(true),
        "None" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod checked_type_tests {
    use super::*;
    use rsscript_semantics::{ResolvedType, TypeQualifiers};

    #[test]
    fn checked_type_conversion_preserves_structure_without_display_parsing() {
        let mut ty = ResolvedType::named(
            "host.models.Page",
            [ResolvedType::named(
                "List",
                [ResolvedType::named("String", [])],
            )],
        );
        ty.qualifiers = TypeQualifiers {
            fresh: true,
            noescape: true,
            owned: true,
        };

        assert_eq!(
            checked_type_to_wire(&ty, "main").expect("named semantic type is wire-representable"),
            WireType::Qualified {
                qualifier: WireQualifier::Fresh,
                value: Box::new(WireType::Qualified {
                    qualifier: WireQualifier::NoEscape,
                    value: Box::new(WireType::Named {
                        package: Some("host.models".into()),
                        name: "Page".into(),
                        arguments: vec![WireType::List {
                            element: Box::new(WireType::String),
                        }],
                    }),
                }),
            }
        );
    }

    #[test]
    fn checked_function_type_fails_closed_until_the_wire_abi_supports_it() {
        let ty = ResolvedType::function(
            [ResolvedType::named("Int", [])],
            [None],
            Some(ResolvedType::named("Int", [])),
            TypeQualifiers::default(),
        );
        assert!(matches!(
            checked_type_to_wire(&ty, "main"),
            Err(MirLoweringError::Unsupported {
                construct: "function type in direct MIR signature",
                ..
            })
        ));
    }

    #[test]
    fn resource_identity_comes_from_a_resolved_named_type_not_rendered_text() {
        assert_eq!(
            resource_type_name_from_wire(&WireType::Named {
                package: Some("host.fs".into()),
                name: "File".into(),
                arguments: Vec::new(),
            }),
            Some("host.fs.File".into())
        );
        assert_eq!(
            resource_type_name_from_wire(&WireType::Qualified {
                qualifier: WireQualifier::Owned,
                value: Box::new(WireType::Resource {
                    name: "host.fs.File".into(),
                }),
            }),
            Some("host.fs.File".into())
        );
        assert_eq!(resource_type_name_from_wire(&WireType::String), None);
    }
}
