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

impl<'source, 'types, 'closures> CheckedHirLowerer<'source, 'types, 'closures> {
    fn new(
        input: CheckedHirLowererInput<'source>,
        types: &'types mut TypeTable,
        closure_registry: &'closures mut ClosureRegistry,
    ) -> Self {
        let CheckedHirLowererInput {
            id,
            function_name,
            body,
            mir_signature,
            initial_places,
            captures,
            targets,
        } = input;
        let mut lowerer = Self {
            id,
            function_name,
            body,
            mir_signature,
            captures,
            targets,
            types,
            closure_registry,
            blocks: vec![BlockDraft::new()],
            current: BlockId::new(0),
            places: HashMap::new(),
            place_types: HashMap::new(),
            closure_abis: HashMap::new(),
            place_names: Vec::new(),
            instruction_sources: Vec::new(),
            next_value: 0,
            tasks: HashMap::new(),
            next_task: 0,
            loops: Vec::new(),
            resource_scopes: Vec::new(),
        };
        for (name, ty) in initial_places {
            lowerer.place_with_type(&name, ty);
        }
        lowerer
    }

    fn lower(mut self) -> Result<LoweredFunction, MirLoweringError> {
        for statement in &self.body.statements {
            if self.current_block().terminator.is_some() {
                return self.unsupported("statement after return");
            }
            self.lower_statement(statement)?;
        }
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Return(None));
        }
        let blocks = self
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| {
                BasicBlock::new(
                    BlockId::new(index as u32),
                    block.instructions,
                    block.terminator.unwrap_or(MirTerminator::Unreachable),
                )
            })
            .collect();
        Ok(LoweredFunction {
            function: MirFunction::with_captures(
                self.id,
                self.mir_signature,
                self.captures,
                self.place_names.len() as u32,
                self.next_value,
                blocks,
            ),
            debug: MirFunctionDebug::with_source(
                self.function_name.to_owned(),
                self.place_names,
                MirSourceLocation::new(
                    self.body.span.file.clone(),
                    self.body.span.line,
                    self.body.span.column,
                    self.body.span.length,
                ),
            )
            .with_instruction_sources(self.instruction_sources),
        })
    }

    fn lower_statement(&mut self, statement: &checked::HirStmt) -> Result<(), MirLoweringError> {
        match statement {
            checked::HirStmt::Let {
                name,
                value,
                ty,
                is_async: false,
                ..
            } => {
                let place = self.place(name);
                if let Some(checked::HirExpr::Closure { ty: Some(ty), .. }) = value {
                    let signature = checked_closure_signature(self.types, ty, &self.function_name)?;
                    self.closure_abis
                        .insert(name.clone(), ClosureAbi::from(&signature));
                }
                if let Some(ty) = ty
                    && let Ok(wire) = checked_type_to_wire(ty, &self.function_name)
                {
                    let ty = self.types.intern(wire);
                    self.place_types.insert(name.clone(), ty);
                }
                if let Some(value) = value {
                    let value = self.lower_expression(value)?;
                    self.emit(MirInstruction::WritePlace { place, value });
                }
                Ok(())
            }
            checked::HirStmt::Return { value, .. } => {
                let value = value
                    .as_ref()
                    .map(|value| self.lower_expression(value))
                    .transpose()?;
                self.emit_resource_cleanup_from(0);
                self.terminate(MirTerminator::Return(value));
                Ok(())
            }
            checked::HirStmt::Assign { target, value, .. } => {
                let value = self.lower_expression(value)?;
                self.lower_assignment_target(target, value)
            }
            checked::HirStmt::Expr(expression) => {
                let value = self.lower_expression(expression)?;
                self.emit(MirInstruction::Discard { value });
                Ok(())
            }
            checked::HirStmt::Let {
                name,
                value,
                is_async: true,
                ..
            } => self.lower_async_binding(name, value.as_ref()),
            checked::HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => self.lower_if(condition, then_body, else_body.as_ref()),
            checked::HirStmt::Loop {
                condition, body, ..
            } => self.lower_loop(condition.as_ref(), body),
            checked::HirStmt::With {
                resource,
                resource_type,
                binding,
                body,
                ..
            } => self.lower_with(resource, resource_type.as_ref(), binding, body),
            checked::HirStmt::For {
                binding,
                iterable,
                iterable_type,
                is_async,
                body,
                ..
            } => self.lower_for(binding, iterable, iterable_type.as_ref(), *is_async, body),
            checked::HirStmt::Match { value, arms, .. } => self.lower_match(value, arms),
            checked::HirStmt::Select { arms, .. } => self.lower_select(arms),
            checked::HirStmt::Break(_) => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("checked HIR break outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.break_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
                Ok(())
            }
            checked::HirStmt::Continue(_) => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("checked HIR continue outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.continue_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
                Ok(())
            }
            checked::HirStmt::Unknown(_) => self.unsupported("unknown checked HIR statement"),
        }
    }

    /// Lower assignment as an explicit rebuild chain. A field assignment first
    /// produces the updated aggregate and then assigns that value to its base;
    /// this preserves value semantics for nested paths without asking a
    /// backend to inspect source-shaped assignment syntax.
    fn lower_assignment_target(
        &mut self,
        target: &checked::HirExpr,
        value: ValueId,
    ) -> Result<(), MirLoweringError> {
        match target {
            checked::HirExpr::Ident { name, .. } => {
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::WritePlace { place, value });
                Ok(())
            }
            checked::HirExpr::Field { base, name, .. } => {
                let base_value = self.lower_expression(base)?;
                self.emit(MirInstruction::SetField {
                    base: base_value,
                    field: name.clone(),
                    value,
                });
                self.lower_assignment_target(base, base_value)
            }
            _ => self.unsupported("non-place checked HIR assignment"),
        }
    }

    fn lower_expression(
        &mut self,
        expression: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        match expression {
            checked::HirExpr::Ident { name, span, .. } if name == "Unit" => self
                .literal_with_source(
                    MirLiteral::Unit,
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                ),
            checked::HirExpr::Ident { name, span, .. } if name == "true" => self
                .literal_with_source(
                    MirLiteral::Bool(true),
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                ),
            checked::HirExpr::Ident { name, span, .. } if name == "false" => self
                .literal_with_source(
                    MirLiteral::Bool(false),
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                ),
            checked::HirExpr::Ident { name, .. } => {
                let destination = self.value();
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::ReadPlace { destination, place });
                Ok(destination)
            }
            checked::HirExpr::Number { value, span } => {
                let value = value
                    .parse::<i64>()
                    .map(MirLiteral::Int)
                    .or_else(|_| value.parse::<f64>().map(MirLiteral::Float))
                    .map_err(|_| MirLoweringError::Unsupported {
                        function: self.function_name.to_owned(),
                        construct: "non-numeric checked HIR literal",
                    })?;
                self.literal_with_source(
                    value,
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                )
            }
            checked::HirExpr::String { value, span } => self.literal_with_source(
                MirLiteral::String(decode_string_token(value)),
                MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
            ),
            checked::HirExpr::Char { value, span } => self.literal_with_source(
                MirLiteral::Char(decode_char_token(value)),
                MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
            ),
            checked::HirExpr::Binary {
                op:
                    op @ (rsscript_syntax::ast::BinaryOp::LogicalAnd
                    | rsscript_syntax::ast::BinaryOp::LogicalOr),
                left,
                right,
                ..
            } => self.lower_logical_binary(*op, left, right),
            checked::HirExpr::Binary {
                op, left, right, ..
            } => {
                let left = self.lower_expression(left)?;
                let right = self.lower_expression(right)?;
                let destination = self.value();
                self.emit(MirInstruction::Binary {
                    destination,
                    op: checked_binary_op(*op),
                    left,
                    right,
                });
                Ok(destination)
            }
            checked::HirExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_expression(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeList { destination, items });
                Ok(destination)
            }
            checked::HirExpr::MapLiteral { entries, .. } => {
                let entries = entries
                    .iter()
                    .map(|entry| -> Result<(ValueId, ValueId), MirLoweringError> {
                        Ok((
                            self.lower_expression(&entry.key)?,
                            self.lower_expression(&entry.value)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeMap {
                    destination,
                    entries,
                });
                Ok(destination)
            }
            checked::HirExpr::ObjectLiteral { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| -> Result<(String, ValueId), MirLoweringError> {
                        Ok((field.name.clone(), self.lower_expression(&field.value)?))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeObject {
                    destination,
                    fields,
                });
                Ok(destination)
            }
            checked::HirExpr::Index {
                base,
                index,
                base_type,
                ..
            } if base_type
                .as_ref()
                .is_some_and(|ty| ty.root_name() == Some("List")) =>
            {
                let list = self.lower_expression(base)?;
                let index = self.lower_expression(index)?;
                let destination = self.value();
                self.emit(MirInstruction::ListGet {
                    destination,
                    list,
                    index,
                });
                Ok(destination)
            }
            checked::HirExpr::Index {
                base,
                index,
                base_type,
                ..
            } if base_type
                .as_ref()
                .is_some_and(|ty| ty.root_name() == Some("Map")) =>
            {
                let map = self.lower_expression(base)?;
                let key = self.lower_expression(index)?;
                let destination = self.value();
                self.emit(MirInstruction::MapGet {
                    destination,
                    map,
                    key,
                });
                Ok(destination)
            }
            checked::HirExpr::Index { .. } => self.unsupported("non-list checked HIR index"),
            checked::HirExpr::Call {
                callee,
                receiver,
                args,
                type_arguments,
                resolution,
                ..
            } => {
                self.lower_direct_call(callee, receiver.as_ref(), args, type_arguments, resolution)
            }
            checked::HirExpr::Effect {
                effect: checked::ParamEffect::Read,
                value,
                ..
            } => self.lower_expression(value),
            checked::HirExpr::Effect {
                effect: checked::ParamEffect::Take,
                value,
                ..
            } => self.lower_take(value),
            checked::HirExpr::Effect { .. } => self.unsupported("mutable checked HIR effect"),
            checked::HirExpr::Manage { value, .. } => self.lower_manage(value),
            checked::HirExpr::Spawn { .. } => self.unsupported("checked HIR spawn"),
            checked::HirExpr::Await { value, .. } => self.lower_await(value),
            checked::HirExpr::Try { value, .. } => {
                let source = self.lower_expression(value)?;
                let destination = self.value();
                self.emit(MirInstruction::TryResult {
                    destination,
                    source,
                    cleanup: self.resource_cleanup_places(),
                });
                Ok(destination)
            }
            checked::HirExpr::Closure {
                params,
                captures,
                ty,
                body,
                ..
            } => self.lower_closure(params, captures, ty.as_ref(), body),
            checked::HirExpr::Field { base, name, .. } => {
                let base = self.lower_expression(base)?;
                let destination = self.value();
                self.emit(MirInstruction::GetField {
                    destination,
                    base,
                    field: name.clone(),
                });
                Ok(destination)
            }
            checked::HirExpr::Match { value, arms, .. } => self.lower_match_expression(value, arms),
            checked::HirExpr::Unknown(_) => self.unsupported("unknown checked HIR expression"),
        }
    }

    /// Lower an owned checked closure into a synthetic MIR function plus an
    /// explicit verifier-visible environment. The source closure never leaks
    /// into a backend: its ABI is the structural HIR `Fn` contract and every
    /// captured local is represented by a typed ownership-mode argument.
    fn lower_closure(
        &mut self,
        params: &[String],
        captures: &[checked::HirClosureCapture],
        ty: Option<&rsscript_semantics::ResolvedType>,
        body: &checked::HirBlock,
    ) -> Result<ValueId, MirLoweringError> {
        let ty = ty.ok_or_else(|| MirLoweringError::Unsupported {
            function: self.function_name.to_owned(),
            construct: "checked HIR closure without structural Fn contract",
        })?;
        let signature = checked_closure_signature(self.types, ty, &self.function_name)?;
        if signature.parameter_types().len() != params.len() {
            return self.unsupported("checked HIR closure parameter/contract arity mismatch");
        }

        let mut initial_places = Vec::with_capacity(captures.len() + params.len());
        let mut mir_captures = Vec::with_capacity(captures.len());
        let mut capture_arguments = Vec::with_capacity(captures.len());
        for capture in captures {
            let place = self.lookup_place(&capture.name)?;
            let ty = self.place_type(&capture.name)?;
            let mode = match capture.effect {
                checked::ParamEffect::Read => MirParameterMode::Read,
                checked::ParamEffect::Mut => MirParameterMode::Mut,
                checked::ParamEffect::Take => MirParameterMode::Take,
            };
            initial_places.push((capture.name.clone(), ty));
            mir_captures.push(rsscript_mir::MirClosureCapture::new(ty, mode));
            capture_arguments.push(match mode {
                MirParameterMode::Read => MirCallArgument::BorrowRead(place),
                MirParameterMode::Mut => MirCallArgument::BorrowMut(place),
                MirParameterMode::Take => MirCallArgument::Take(place),
            });
        }
        for (name, ty) in params
            .iter()
            .cloned()
            .zip(signature.parameter_types().iter().copied())
        {
            if initial_places.iter().any(|(existing, _)| existing == &name) {
                return self.unsupported("checked HIR closure capture shadows parameter");
            }
            initial_places.push((name, ty));
        }

        let id = self.closure_registry.allocate();
        let closure_name = format!("{}::<closure:{}>", self.function_name, id.index());
        let output = CheckedHirLowerer::new(
            CheckedHirLowererInput {
                id,
                function_name: closure_name,
                body,
                mir_signature: signature,
                initial_places,
                captures: mir_captures,
                targets: self.targets.clone(),
            },
            self.types,
            self.closure_registry,
        )
        .lower()?;
        self.closure_registry.push(output);

        let destination = self.value();
        self.emit(MirInstruction::MakeClosure {
            destination,
            function: id,
            captures: capture_arguments,
        });
        Ok(destination)
    }

    /// Lower boolean `&&`/`||` as explicit CFG rather than a binary opcode.
    ///
    /// Short-circuiting is an observable execution property: evaluating the
    /// right side before branching could invoke a Provider, allocate a
    /// resource, or fail even when the result is already known. Keeping both
    /// paths explicit also means bytecode codegen needs only ordinary branch,
    /// write, and read operations; it never has to recover source-level
    /// short-circuit behavior from a generic binary instruction.
    fn lower_logical_binary(
        &mut self,
        op: rsscript_syntax::ast::BinaryOp,
        left: &checked::HirExpr,
        right: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        let left = self.lower_expression(left)?;
        let right_block = self.new_block();
        let short_circuit_block = self.new_block();
        let join_block = self.new_block();
        let result_place = self.place(&format!("__rss_mir_logical_result_{}", self.next_value));
        let short_circuit_value = match op {
            rsscript_syntax::ast::BinaryOp::LogicalAnd => false,
            rsscript_syntax::ast::BinaryOp::LogicalOr => true,
            _ => return self.unsupported("non-logical checked HIR binary operation"),
        };
        let (then_target, else_target) = match op {
            rsscript_syntax::ast::BinaryOp::LogicalAnd => (right_block, short_circuit_block),
            rsscript_syntax::ast::BinaryOp::LogicalOr => (short_circuit_block, right_block),
            _ => return self.unsupported("non-logical checked HIR binary operation"),
        };
        self.terminate(MirTerminator::Branch {
            condition: left,
            then_target,
            else_target,
        });

        self.current = right_block;
        let right = self.lower_expression(right)?;
        self.emit(MirInstruction::WritePlace {
            place: result_place,
            value: right,
        });
        self.terminate(MirTerminator::Jump(join_block));

        self.current = short_circuit_block;
        let value = self.literal(MirLiteral::Bool(short_circuit_value))?;
        self.emit(MirInstruction::WritePlace {
            place: result_place,
            value,
        });
        self.terminate(MirTerminator::Jump(join_block));

        self.current = join_block;
        let destination = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination,
            place: result_place,
        });
        Ok(destination)
    }

    fn literal(&mut self, value: MirLiteral) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        self.emit(MirInstruction::LoadLiteral { destination, value });
        Ok(destination)
    }

    fn literal_with_source(
        &mut self,
        value: MirLiteral,
        source: MirSourceLocation,
    ) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        let block = self.current;
        let instruction_index = u32::try_from(self.current_block().instructions.len())
            .expect("RSScript MIR instruction count exceeds the u32 source-map address space");
        self.emit(MirInstruction::LoadLiteral { destination, value });
        self.instruction_sources
            .push(MirInstructionSource::new(block, instruction_index, source));
        Ok(destination)
    }

    fn lower_direct_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        receiver: Option<&checked::HirCallReceiver>,
        args: &[checked::HirCallArg],
        type_arguments: &[ResolvedType],
        resolution: &checked::CallResolution,
    ) -> Result<ValueId, MirLoweringError> {
        if matches!(resolution, checked::CallResolution::EnumVariant) {
            if receiver.is_some() {
                return self.unsupported("checked HIR receiver enum-variant call");
            }
            return self.lower_enum_variant_call(callee, args);
        }
        if matches!(resolution, checked::CallResolution::Unknown)
            && receiver.is_none()
            && let rsscript_syntax::ast::Callee::Name(name) = callee
            && let Some(abi) = self.closure_abis.get(name).cloned()
        {
            return self.lower_local_closure_call(name, args, abi);
        }
        let checked::CallResolution::Resolved { signature, kind } = resolution else {
            return self.unsupported("unresolved checked HIR call");
        };
        if matches!(
            kind,
            checked::ResolvedCalleeKind::Constructor {
                type_kind: checked::HirTypeKind::Struct | checked::HirTypeKind::Class,
            }
        ) {
            return self.lower_record_constructor(signature, args);
        }
        if matches!(kind, checked::ResolvedCalleeKind::Constructor { .. }) {
            return self.unsupported("non-record checked HIR constructor");
        }
        // Core interfaces mark their signatures as builtins directly. The
        // explicit async standard package exposes the same deterministic VM
        // operations as `.rssi` declarations, however, so its checked
        // signatures are interface-shaped. A catalog hit is the canonical
        // identity in both forms: route it through `BuiltinId` rather than
        // creating a fictitious Provider import for a VM-owned operation.
        if signature.is_builtin || is_catalog_builtin(signature) {
            return self.lower_builtin_call(callee, signature, receiver, args);
        }
        let target = if let Some(dispatch) = signature.namespace.as_deref().and_then(|namespace| {
            self.targets
                .dynamic_protocol_methods
                .get(&(namespace.to_owned(), signature.name.clone()))
        }) {
            MirCallTarget::Dynamic {
                dispatch: dispatch.clone(),
                parameter_modes: checked_parameter_modes(signature).into_boxed_slice(),
            }
        } else if signature.is_external {
            let symbol = checked_external_symbol(signature)?;
            self.targets
                .external_imports
                .get(symbol.as_str())
                .copied()
                .map(MirCallTarget::External)
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "direct checked HIR external call target",
                })?
        } else {
            let qualified = signature
                .namespace
                .as_ref()
                .map(|namespace| format!("{namespace}.{}", signature.name));
            let function = self
                .targets
                .functions
                .get(&signature.name)
                .or_else(|| {
                    qualified
                        .as_ref()
                        .and_then(|name| self.targets.functions.get(name))
                })
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "direct checked HIR call target",
                })?;
            if type_arguments.is_empty() {
                MirCallTarget::Function(function)
            } else {
                let concrete_arguments = type_arguments
                    .iter()
                    .map(|ty| checked_type_to_wire(ty, &self.function_name))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|ty| self.types.intern(ty))
                    .collect::<Vec<_>>();
                let type_substitutions = signature
                    .type_params
                    .iter()
                    .zip(concrete_arguments)
                    .map(|(parameter, argument)| {
                        let parameter = self.types.intern(WireType::Named {
                            package: None,
                            name: parameter.clone(),
                            arguments: Vec::new(),
                        });
                        (parameter, argument)
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                MirCallTarget::FunctionInstance {
                    function,
                    type_substitutions,
                }
            }
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let mut arguments = Vec::with_capacity(ordered.len() + usize::from(receiver.is_some()));
        let mut retained_places = Vec::new();
        if let Some(receiver) = receiver {
            let lowered = self.lower_direct_receiver_argument(receiver)?;
            if signature
                .params
                .first()
                .is_some_and(|parameter| signature.retained_params.contains(&parameter.name))
                && let MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) =
                    lowered
            {
                retained_places.push(place);
            }
            arguments.push(lowered);
        }
        for argument in ordered {
            let lowered = self.lower_direct_call_argument(&argument.value)?;
            if argument
                .parameter_index
                .and_then(|index| signature.params.get(index))
                .is_some_and(|parameter| signature.retained_params.contains(&parameter.name))
                && let MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) =
                    lowered
            {
                retained_places.push(place);
            }
            arguments.push(lowered);
        }
        let destination = self.value();
        self.emit(MirInstruction::Call {
            destination,
            target,
            arguments,
        });
        for place in retained_places {
            self.emit(MirInstruction::Retain { place });
        }
        Ok(destination)
    }

    /// Lower an invocation through a local first-class closure value. The
    /// closure's concrete synthetic function remains opaque here; its typed
    /// parameter contract was recorded when the binding was constructed.
    fn lower_local_closure_call(
        &mut self,
        name: &str,
        args: &[checked::HirCallArg],
        abi: ClosureAbi,
    ) -> Result<ValueId, MirLoweringError> {
        let place = self.lookup_place(name)?;
        let closure = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination: closure,
            place,
        });
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_direct_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let destination = self.value();
        self.emit(MirInstruction::CallClosure {
            destination,
            closure,
            parameter_types: abi.parameter_types,
            parameter_modes: abi.parameter_modes,
            arguments,
        });
        Ok(destination)
    }

    /// Materialize a resolved struct/class constructor directly from checked
    /// signature facts. Arguments still evaluate in source order, while the
    /// resulting layout fields use declaration/parameter order.
    fn lower_record_constructor(
        &mut self,
        signature: &checked::FunctionSig,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        let wire_type = signature
            .return_ty
            .as_ref()
            .map(|ty| checked_type_to_wire(ty, &self.function_name))
            .transpose()?
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "record constructor without result type",
            })?;
        if !matches!(wire_type, WireType::Named { .. }) {
            return self.unsupported("record constructor with non-named result type");
        }
        let mut values = vec![None; signature.params.len()];
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        for argument in ordered {
            let index = argument
                .parameter_index
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "record constructor with unresolved argument binding",
                })?;
            let Some(parameter) = signature.params.get(index) else {
                return self.unsupported("record constructor argument outside signature");
            };
            if values[index].is_some() {
                return self.unsupported("record constructor duplicate argument binding");
            }
            values[index] = Some((
                parameter.name.clone(),
                self.lower_expression(&argument.value)?,
            ));
        }
        let fields = values
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "record constructor missing checked field binding",
            })?;
        let destination = self.value();
        let ty = self.types.intern(wire_type);
        self.emit(MirInstruction::MakeStruct {
            destination,
            ty,
            fields,
        });
        Ok(destination)
    }

    /// `Ok` and `Err` are language result constructors, not normal runtime
    /// intrinsics. Lower them into a typed operation so VM codegen never has
    /// to rediscover a source-level builtin name.
    fn lower_builtin_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        signature: &checked::FunctionSig,
        receiver: Option<&checked::HirCallReceiver>,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        // JSON decode is the current builtin whose concrete type argument
        // changes runtime behavior. Its type operand is preserved below;
        // generic channel payloads are already fully checked and phantom to
        // the VM's channel state, so they retain the ordinary `BuiltinId`.
        let destination = self.value();
        match signature.name.as_str() {
            "Ok" | "Err" => {
                if receiver.is_some() {
                    return self.unsupported("Result constructor receiver call");
                }
                if args.len() != 1 {
                    return self.unsupported("Result constructor with non-unary arity");
                }
                let value = self.lower_expression(&args[0].value)?;
                self.emit(MirInstruction::MakeResult {
                    destination,
                    ok: signature.name == "Ok",
                    value,
                });
            }
            "Some" => {
                if receiver.is_some() {
                    return self.unsupported("Option Some receiver call");
                }
                if args.len() != 1 {
                    return self.unsupported("Option Some constructor with non-unary arity");
                }
                let value = self.lower_expression(&args[0].value)?;
                self.emit(MirInstruction::MakeOption {
                    destination,
                    value: Some(value),
                });
            }
            "None" => {
                if receiver.is_some() {
                    return self.unsupported("Option None receiver call");
                }
                if !args.is_empty() {
                    return self.unsupported("Option None constructor with non-zero arity");
                }
                self.emit(MirInstruction::MakeOption {
                    destination,
                    value: None,
                });
            }
            "concat" if signature.namespace.as_deref() == Some("String") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("String.concat with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let left = self.lower_expression(&ordered[0].value)?;
                let right = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::StringConcat {
                    destination,
                    left,
                    right,
                });
            }
            "get" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.get with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_expression(&ordered[0].value)?;
                let index = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::ListGet {
                    destination,
                    list,
                    index,
                });
            }
            "len" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("List.len with invalid checked call shape");
                }
                let list = self.lower_expression(&args[0].value)?;
                self.emit(MirInstruction::ListLen { destination, list });
            }
            "append" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.append with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (values, retained_values) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::ListAppend {
                    destination,
                    list,
                    values,
                });
                if let Some(place) = retained_values {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "clear" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("List.clear with invalid checked call shape");
                }
                let list = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::ListClear { destination, list });
            }
            "pop" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("List.pop with invalid checked call shape");
                }
                let list = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::ListPop { destination, list });
            }
            "push" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.push with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::ListPush {
                    destination,
                    list,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove_at" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.remove_at with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let index = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::ListRemoveAt {
                    destination,
                    list,
                    index,
                });
            }
            "set" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("List.set with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let index = self.lower_expression(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::ListSet {
                    destination,
                    list,
                    index,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "clear" if signature.namespace.as_deref() == Some("Set") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Set.clear with invalid checked call shape");
                }
                let set = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::SetClear { destination, set });
            }
            "insert" if signature.namespace.as_deref() == Some("Set") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Set.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::SetInsert {
                    destination,
                    set,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("Set") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Set.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let value = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::SetRemove {
                    destination,
                    set,
                    value,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Deque.clear with invalid checked call shape");
                }
                let deque = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::DequeClear { destination, deque });
            }
            "pop_back" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Deque.pop_back with invalid checked call shape");
                }
                let deque = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::DequePopBack { destination, deque });
            }
            "pop_front" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Deque.pop_front with invalid checked call shape");
                }
                let deque = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::DequePopFront { destination, deque });
            }
            "push_back" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Deque.push_back with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let deque = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::DequePushBack {
                    destination,
                    deque,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "push_front" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Deque.push_front with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let deque = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::DequePushFront {
                    destination,
                    deque,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "clear" if signature.namespace.as_deref() == Some("SortedMap") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("SortedMap.clear with invalid checked call shape");
                }
                let map = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::SortedMapClear { destination, map });
            }
            "insert" if signature.namespace.as_deref() == Some("SortedMap") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("SortedMap.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (key, retained_key) = self.lower_retained_builtin_value(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::SortedMapInsert {
                    destination,
                    map,
                    key,
                    value,
                });
                for place in [retained_key, retained_value].into_iter().flatten() {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("SortedMap") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("SortedMap.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let key = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::SortedMapRemove {
                    destination,
                    map,
                    key,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("SortedSet") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("SortedSet.clear with invalid checked call shape");
                }
                let set = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::SortedSetClear { destination, set });
            }
            "insert" if signature.namespace.as_deref() == Some("SortedSet") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("SortedSet.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::SortedSetInsert {
                    destination,
                    set,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("SortedSet") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("SortedSet.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let value = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::SortedSetRemove {
                    destination,
                    set,
                    value,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("Buffer") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Buffer.clear with invalid checked call shape");
                }
                let buffer = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::BufferClear {
                    destination,
                    buffer,
                });
            }
            "push" if signature.namespace.as_deref() == Some("StringBuilder") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("StringBuilder.push with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let builder = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let value = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::StringBuilderPush {
                    destination,
                    builder,
                    value,
                });
            }
            "finish" if signature.namespace.as_deref() == Some("StringBuilder") => {
                if receiver.is_some() || args.len() != 1 {
                    return self
                        .unsupported("StringBuilder.finish with invalid checked call shape");
                }
                let checked::HirExpr::Effect {
                    effect: checked::ParamEffect::Take,
                    value,
                    ..
                } = &args[0].value
                else {
                    return self
                        .unsupported("StringBuilder.finish without checked take argument effect");
                };
                let builder = self.lower_take(value)?;
                self.emit(MirInstruction::StringBuilderFinish {
                    destination,
                    builder,
                });
            }
            "get" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Map.get with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_expression(&ordered[0].value)?;
                let key = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::MapGet {
                    destination,
                    map,
                    key,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Map.clear with invalid checked call shape");
                }
                let map = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::MapClear { destination, map });
            }
            "insert" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("Map.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (key, retained_key) = self.lower_retained_builtin_value(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::MapInsert {
                    destination,
                    map,
                    key,
                    value,
                });
                for place in [retained_key, retained_value].into_iter().flatten() {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "insert_old" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("Map.insert_old with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (key, retained_key) = self.lower_retained_builtin_value(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::MapInsertOld {
                    destination,
                    map,
                    key,
                    value,
                });
                for place in [retained_key, retained_value].into_iter().flatten() {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Map.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let key = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::MapRemove {
                    destination,
                    map,
                    key,
                });
            }
            _ => {
                let Some(namespace) = signature.namespace.as_deref() else {
                    return self.unsupported("builtin checked HIR call without namespace");
                };
                let Some(builtin) = rsscript_mir::builtin_id(namespace, &signature.name) else {
                    return self.unsupported("unsupported checked HIR builtin call");
                };
                let type_arguments = if is_json_decode_builtin(signature) {
                    self.json_decode_type_arguments(callee)?
                } else {
                    Vec::new()
                };
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let mut arguments =
                    Vec::with_capacity(ordered.len() + usize::from(receiver.is_some()));
                if let Some(receiver) = receiver {
                    arguments.push(self.lower_direct_receiver_argument(receiver)?);
                }
                arguments.extend(
                    ordered
                        .into_iter()
                        .map(|argument| self.lower_direct_call_argument(&argument.value))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                self.emit(MirInstruction::Call {
                    destination,
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
                        type_arguments: type_arguments.into_boxed_slice(),
                    },
                    arguments,
                });
            }
        }
        Ok(destination)
    }

    /// JSON decode has a semantic type argument which changes the VM's
    /// decoding contract. Preserve it as a typed MIR operand instead of
    /// letting the compatibility executable IR recover it from the callee
    /// spelling at backend time. The checked HIR still carries this spelling
    /// during the transition; it is converted to canonical `WireType` before
    /// it enters MIR.
    fn json_decode_type_arguments(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
    ) -> Result<Vec<TypeId>, MirLoweringError> {
        let type_argument = callee_type_arguments(callee)
            .and_then(|arguments| arguments.first().copied())
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "JSON decode without concrete type argument",
            })?;
        if callee_type_arguments(callee).is_some_and(|arguments| arguments.len() != 1) {
            return self.unsupported("JSON decode with invalid type argument arity");
        }
        Ok(vec![self.types.intern(WireType::parse(type_argument))])
    }

    fn lower_enum_variant_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        let name = match callee {
            rsscript_syntax::ast::Callee::Name(name)
            | rsscript_syntax::ast::Callee::Qualified { name, .. } => {
                name.split('<').next().unwrap_or(name).trim()
            }
            rsscript_syntax::ast::Callee::ReceiverCall { .. } => {
                return self.unsupported("checked HIR receiver enum variant call");
            }
        };
        if let Some(ok) = match name {
            "Ok" => Some(true),
            "Err" => Some(false),
            _ => None,
        } {
            if args.len() != 1 {
                return self.unsupported("Result enum variant with non-unary arity");
            }
            let value = self.lower_expression(&args[0].value)?;
            let destination = self.value();
            self.emit(MirInstruction::MakeResult {
                destination,
                ok,
                value,
            });
            return Ok(destination);
        }
        if let Some(some) = option_variant_tag(name) {
            let destination = self.value();
            match some {
                true => {
                    if args.len() != 1 {
                        return self.unsupported("Option Some variant with non-unary arity");
                    }
                    let value = self.lower_expression(&args[0].value)?;
                    self.emit(MirInstruction::MakeOption {
                        destination,
                        value: Some(value),
                    });
                }
                false => {
                    if !args.is_empty() {
                        return self.unsupported("Option None variant with non-zero arity");
                    }
                    self.emit(MirInstruction::MakeOption {
                        destination,
                        value: None,
                    });
                }
            }
            return Ok(destination);
        }
        let Some(layout) = self.targets.variants.get(name).cloned() else {
            return self.unsupported("unknown checked HIR enum variant");
        };
        let mut values = vec![None; layout.fields.len()];
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        for argument in ordered {
            let index = argument
                .parameter_index
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "enum variant with unresolved argument binding",
                })?;
            if index >= values.len() || values[index].is_some() {
                return self.unsupported("enum variant with invalid argument binding");
            }
            values[index] = Some(self.lower_expression(&argument.value)?);
        }
        let fields = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| value.map(|value| (layout.fields[index].clone(), value)))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "enum variant missing checked field binding",
            })?;
        let destination = self.value();
        let ty = self.types.intern(WireType::Named {
            package: None,
            name: layout.owner,
            arguments: Vec::new(),
        });
        self.emit(MirInstruction::MakeVariant {
            destination,
            ty,
            variant: name.to_owned(),
            fields,
        });
        Ok(destination)
    }

    fn lower_take(&mut self, value: &checked::HirExpr) -> Result<ValueId, MirLoweringError> {
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("take checked HIR effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        let destination = self.value();
        self.emit(MirInstruction::TakePlace { destination, place });
        Ok(destination)
    }

    /// A mutating builtin must carry the checked mutable place directly. This
    /// is intentionally narrower than general mutable argument lowering: each
    /// new in-place MIR operation decides its own runtime contract instead of
    /// erasing `mut` into an ordinary value before codegen.
    fn lower_mutable_builtin_place(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<PlaceId, MirLoweringError> {
        let checked::HirExpr::Effect {
            effect: checked::ParamEffect::Mut,
            value,
            ..
        } = value
        else {
            return self.unsupported("mutating builtin argument without checked mut effect");
        };
        self.lower_mutable_place(value)
    }

    /// A checked `mut` argument is usually a local place. Struct fields are
    /// also valid mutable collection locations: materialize the field value in
    /// a compiler-private place so the existing collection MIR instructions
    /// retain their explicit mutation operand while the runtime continues to
    /// mutate the shared collection identity.
    fn lower_mutable_place(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<PlaceId, MirLoweringError> {
        match value {
            checked::HirExpr::Ident { name, .. } => self.lookup_place(name),
            checked::HirExpr::Field { .. } => {
                let source = self.lower_expression(value)?;
                let place = self.place(&format!("$mir_mut_field_{}", self.place_names.len()));
                self.emit(MirInstruction::WritePlace {
                    place,
                    value: source,
                });
                Ok(place)
            }
            _ => self.unsupported("mutating builtin argument on non-place value"),
        }
    }

    /// Materialize a value that the resolved builtin retains and preserve the
    /// semantic retention fact when the caller supplied a `read local`.
    ///
    /// The resulting runtime operand is still a normal `ValueId`; the
    /// separate `Retain` makes the ownership contract visible to the MIR
    /// verifier instead of leaving it encoded only in the `.rssi` signature.
    fn lower_retained_builtin_value(
        &mut self,
        argument: &checked::HirExpr,
    ) -> Result<(ValueId, Option<PlaceId>), MirLoweringError> {
        let retained_place = match argument {
            checked::HirExpr::Effect {
                effect: checked::ParamEffect::Read,
                value,
                ..
            } => match value.as_ref() {
                checked::HirExpr::Ident { name, .. } if !is_checked_literal_ident(value) => {
                    Some(self.lookup_place(name)?)
                }
                _ => None,
            },
            _ => None,
        };
        let value = self.lower_expression(argument)?;
        Ok((value, retained_place))
    }

    /// `manage local` consumes the local graph and creates a stable managed
    /// identity. Keep both operations visible instead of treating `manage` as
    /// a transparent read, otherwise a later local use could bypass the
    /// ownership transition represented by semantic HIR.
    fn lower_manage(&mut self, value: &checked::HirExpr) -> Result<ValueId, MirLoweringError> {
        let source = self.lower_take(value)?;
        let destination = self.value();
        self.emit(MirInstruction::Manage {
            destination,
            source,
        });
        Ok(destination)
    }

    fn lower_direct_call_argument(
        &mut self,
        argument: &checked::HirExpr,
    ) -> Result<MirCallArgument, MirLoweringError> {
        if let checked::HirExpr::Manage { value, .. } = argument {
            let checked::HirExpr::Ident { name, .. } = value.as_ref() else {
                return self.unsupported("manage checked HIR call argument on non-local value");
            };
            return self.lookup_place(name).map(MirCallArgument::BorrowRead);
        }
        let checked::HirExpr::Effect { effect, value, .. } = argument else {
            return self.lower_expression(argument).map(MirCallArgument::Value);
        };
        // `read` is an observation-only qualifier. It may be attached to an
        // rvalue such as a string literal, where there is no caller-owned
        // place to borrow. Preserve that distinction in MIR as an ordinary
        // value argument; only local `read` values use `BorrowRead` so the
        // verifier can track the place lifetime.
        if *effect == checked::ParamEffect::Read {
            match value.as_ref() {
                checked::HirExpr::Ident { name, .. } if !is_checked_literal_ident(value) => {
                    return self.lookup_place(name).map(MirCallArgument::BorrowRead);
                }
                checked::HirExpr::Manage { value, .. } => {
                    let checked::HirExpr::Ident { name, .. } = value.as_ref() else {
                        return self
                            .unsupported("manage checked HIR call argument on non-local value");
                    };
                    return self.lookup_place(name).map(MirCallArgument::BorrowRead);
                }
                _ => return self.lower_expression(value).map(MirCallArgument::Value),
            }
        }
        if *effect == checked::ParamEffect::Mut {
            return self
                .lower_mutable_place(value)
                .map(MirCallArgument::BorrowMut);
        }
        let value = match value.as_ref() {
            checked::HirExpr::Ident { .. } => value.as_ref(),
            _ => return self.unsupported("checked HIR data effect on non-local value"),
        };
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("checked HIR data effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        Ok(match effect {
            checked::ParamEffect::Read | checked::ParamEffect::Mut => {
                unreachable!("read and mut effects returned above")
            }
            checked::ParamEffect::Take => MirCallArgument::Take(place),
        })
    }

    /// Receiver calls are semantically bound as parameter zero. Preserve their
    /// declared effect just like an explicitly named argument, while allowing
    /// a read-qualified rvalue receiver to remain an ordinary owned value.
    fn lower_direct_receiver_argument(
        &mut self,
        receiver: &checked::HirCallReceiver,
    ) -> Result<MirCallArgument, MirLoweringError> {
        let value = receiver.value.as_ref();
        match receiver.effect {
            checked::ParamEffect::Read => {
                if let checked::HirExpr::Ident { name, .. } = value
                    && !is_checked_literal_ident(value)
                {
                    return self.lookup_place(name).map(MirCallArgument::BorrowRead);
                }
                self.lower_expression(value).map(MirCallArgument::Value)
            }
            checked::ParamEffect::Mut => {
                let place = self.lower_mutable_place(value)?;
                Ok(MirCallArgument::BorrowMut(place))
            }
            checked::ParamEffect::Take => {
                let checked::HirExpr::Ident { name, .. } = value else {
                    return self.unsupported("checked HIR take receiver on non-local value");
                };
                let place = self.lookup_place(name)?;
                Ok(match receiver.effect {
                    checked::ParamEffect::Take => MirCallArgument::Take(place),
                    checked::ParamEffect::Read => unreachable!("matched above"),
                    checked::ParamEffect::Mut => unreachable!("matched above"),
                })
            }
        }
    }

    fn lower_async_binding(
        &mut self,
        name: &str,
        value: Option<&checked::HirExpr>,
    ) -> Result<(), MirLoweringError> {
        if self.tasks.contains_key(name) {
            return self.unsupported("duplicate async checked HIR binding");
        }
        let Some(value) = value else {
            return self.unsupported("async checked HIR binding without direct call");
        };
        let task = self.lower_spawn_call(value)?;
        self.tasks.insert(name.to_owned(), task);
        Ok(())
    }

    /// Lower one resolved async call into an owned child task. Both `async let`
    /// and `select` use this exact path so they share target resolution,
    /// Provider-wrapper construction, argument ownership checks, and the
    /// lexical task group.
    fn lower_spawn_call(&mut self, value: &checked::HirExpr) -> Result<TaskId, MirLoweringError> {
        let checked::HirExpr::Call {
            receiver,
            args,
            resolution,
            ..
        } = value
        else {
            return self.unsupported("async checked HIR binding without direct call");
        };
        let checked::CallResolution::Resolved { signature, .. } = resolution else {
            return self.unsupported("unresolved async checked HIR call");
        };
        if signature.is_builtin && !is_catalog_builtin(signature) {
            return self.unsupported("async builtin checked HIR call");
        }
        let qualified = signature
            .namespace
            .as_ref()
            .map(|namespace| format!("{namespace}.{}", signature.name));
        let target = if is_catalog_builtin(signature) {
            self.targets
                .async_builtin_wrappers
                .get(
                    qualified
                        .as_deref()
                        .ok_or_else(|| MirLoweringError::Unsupported {
                            function: self.function_name.to_owned(),
                            construct: "async catalog builtin without qualified identity",
                        })?,
                )
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "async catalog builtin checked HIR wrapper",
                })?
        } else if signature.is_external {
            let symbol = checked_external_symbol(signature)?;
            self.targets
                .async_external_wrappers
                .get(symbol.as_str())
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "async external checked HIR wrapper",
                })?
        } else {
            self.targets
                .functions
                .get(&signature.name)
                .or_else(|| {
                    qualified
                        .as_ref()
                        .and_then(|name| self.targets.functions.get(name))
                })
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "direct async checked HIR call target",
                })?
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let mut arguments = Vec::with_capacity(ordered.len() + usize::from(receiver.is_some()));
        if let Some(receiver) = receiver {
            arguments.push(self.lower_direct_receiver_argument(receiver)?);
        }
        arguments.extend(
            ordered
                .into_iter()
                .map(|argument| self.lower_direct_call_argument(&argument.value))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let task = self.task();
        self.emit(MirInstruction::Spawn {
            task,
            group: TaskGroupId::new(0),
            target,
            arguments,
        });
        Ok(task)
    }

    fn lower_await(&mut self, value: &checked::HirExpr) -> Result<ValueId, MirLoweringError> {
        // A checked async external call is already a suspension-capable MIR
        // `CallExternal`: the VM parks the current task while its linked
        // Provider future is pending, then writes the result to the call
        // destination. Do not fabricate an internal task merely to represent
        // `await Host.call()`.
        if let checked::HirExpr::Call {
            callee,
            receiver,
            args,
            type_arguments,
            resolution,
            ..
        } = value
        {
            let checked::CallResolution::Resolved { signature, .. } = resolution else {
                return self.unsupported("unresolved awaited checked HIR call");
            };
            if signature.is_external && signature.is_async {
                return self.lower_direct_call(
                    callee,
                    receiver.as_ref(),
                    args,
                    type_arguments,
                    resolution,
                );
            }
        }
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("await of non-task checked HIR local");
        };
        let Some(task) = self.tasks.get(name).copied() else {
            return self.unsupported("await of unknown checked HIR task");
        };
        let destination = self.value();
        self.emit(MirInstruction::Await { destination, task });
        Ok(destination)
    }

    /// Lower `select` as explicit task creation, one verifier-visible first
    /// ready wait, and ordinary CFG dispatch into the selected arm. The select
    /// instruction consumes every arm task: the VM transfers the winner value
    /// and cancels/reaps all losers before the branch ladder executes.
    fn lower_select(&mut self, arms: &[checked::HirSelectArm]) -> Result<(), MirLoweringError> {
        if arms.is_empty() {
            return Ok(());
        }

        let mut tasks = Vec::with_capacity(arms.len());
        let mut arm_has_try = Vec::with_capacity(arms.len());
        for arm in arms {
            let (operation, has_try) = peel_checked_select_operation(&arm.operation);
            tasks.push(self.lower_spawn_call(operation)?);
            arm_has_try.push(has_try);
        }

        let winner = self.value();
        let value = self.value();
        self.emit(MirInstruction::Select {
            tasks,
            winner,
            value,
        });

        let join = self.new_block();
        for (index, arm) in arms.iter().enumerate() {
            let arm_block = self.new_block();
            let next = self.new_block();
            let arm_index = self.literal(MirLiteral::Int(index as i64))?;
            let matches_arm = self.value();
            self.emit(MirInstruction::Binary {
                destination: matches_arm,
                op: MirBinaryOp::Equal,
                left: winner,
                right: arm_index,
            });
            self.terminate(MirTerminator::Branch {
                condition: matches_arm,
                then_target: arm_block,
                else_target: next,
            });

            self.current = arm_block;
            let bound = if arm_has_try[index] {
                let destination = self.value();
                self.emit(MirInstruction::TryResult {
                    destination,
                    source: value,
                    cleanup: self.resource_cleanup_places(),
                });
                destination
            } else {
                value
            };
            if arm.binding != "_" {
                let binding = self.place(&arm.binding);
                self.emit(MirInstruction::WritePlace {
                    place: binding,
                    value: bound,
                });
            }
            self.lower_checked_block(&arm.body)?;
            if self.current_block().terminator.is_none() {
                self.terminate(MirTerminator::Jump(join));
            }

            self.current = next;
        }
        self.terminate(MirTerminator::Unreachable);
        self.current = join;
        Ok(())
    }

    fn lower_if(
        &mut self,
        condition: &checked::HirExpr,
        then_body: &checked::HirBlock,
        else_body: Option<&checked::HirBlock>,
    ) -> Result<(), MirLoweringError> {
        let condition = self.lower_expression(condition)?;
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join_block = self.new_block();
        self.terminate(MirTerminator::Branch {
            condition,
            then_target: then_block,
            else_target: else_block,
        });

        self.current = then_block;
        self.lower_checked_block(then_body)?;
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = else_block;
        if let Some(else_body) = else_body {
            self.lower_checked_block(else_body)?;
        }
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = join_block;
        Ok(())
    }

    fn lower_match(
        &mut self,
        value: &checked::HirExpr,
        arms: &[checked::HirMatchArm],
    ) -> Result<(), MirLoweringError> {
        let value = self.lower_expression(value)?;
        let join = self.new_block();
        for arm in arms {
            if arm.guard.is_some() {
                return self.unsupported("checked HIR match guard");
            }
            let arm_block = self.new_block();
            let next = self.new_block();
            let variant_bindings = match &arm.pattern {
                rsscript_syntax::ast::MatchPattern::Wildcard(_) => {
                    self.terminate(MirTerminator::Jump(arm_block));
                    None
                }
                rsscript_syntax::ast::MatchPattern::Literal { value: literal, .. } => {
                    let literal = match_literal(literal, &self.function_name)?;
                    let expected = self.literal(literal)?;
                    let condition = self.value();
                    self.emit(MirInstruction::Binary {
                        destination: condition,
                        op: MirBinaryOp::Equal,
                        left: value,
                        right: expected,
                    });
                    self.terminate(MirTerminator::Branch {
                        condition,
                        then_target: arm_block,
                        else_target: next,
                    });
                    None
                }
                rsscript_syntax::ast::MatchPattern::Variant { name, bindings, .. } => {
                    if let Some(ok) = result_variant_tag(name) {
                        let binding = self.result_pattern_binding(bindings)?;
                        self.terminate(MirTerminator::MatchResult {
                            value,
                            ok_target: if ok { arm_block } else { next },
                            err_target: if ok { next } else { arm_block },
                        });
                        Some(MatchBindings::Result { ok, binding })
                    } else if let Some(some) = option_variant_tag(name) {
                        let binding = self.option_pattern_binding(some, bindings)?;
                        self.terminate(MirTerminator::MatchOption {
                            value,
                            some_target: if some { arm_block } else { next },
                            none_target: if some { next } else { arm_block },
                        });
                        Some(MatchBindings::Option { some, binding })
                    } else {
                        let layout = self.variant_pattern_layout(name, bindings)?;
                        self.terminate(MirTerminator::MatchVariant {
                            value,
                            expected: name.clone(),
                            match_target: arm_block,
                            else_target: next,
                        });
                        Some(MatchBindings::Variant(layout, bindings.clone()))
                    }
                }
                _ => return self.unsupported("non-literal checked HIR match pattern"),
            };

            self.current = arm_block;
            if let Some(bindings) = variant_bindings {
                self.lower_match_bindings(value, bindings)?;
            }
            self.lower_checked_block(&arm.body)?;
            if self.current_block().terminator.is_none() {
                self.terminate(MirTerminator::Jump(join));
            }
            self.current = next;
        }
        self.terminate(MirTerminator::Unreachable);
        self.current = join;
        Ok(())
    }

    fn lower_match_expression(
        &mut self,
        value: &checked::HirExpr,
        arms: &[checked::HirMatchArm],
    ) -> Result<ValueId, MirLoweringError> {
        let value = self.lower_expression(value)?;
        let result_place = self.place(&format!("__rss_mir_match_result_{}", self.next_value));
        let join = self.new_block();
        for arm in arms {
            if arm.guard.is_some() {
                return self.unsupported("checked HIR match expression guard");
            }
            let arm_block = self.new_block();
            let next = self.new_block();
            let variant_bindings = match &arm.pattern {
                rsscript_syntax::ast::MatchPattern::Wildcard(_) => {
                    self.terminate(MirTerminator::Jump(arm_block));
                    None
                }
                rsscript_syntax::ast::MatchPattern::Literal { value: literal, .. } => {
                    let expected = self.literal(match_literal(literal, &self.function_name)?)?;
                    let condition = self.value();
                    self.emit(MirInstruction::Binary {
                        destination: condition,
                        op: MirBinaryOp::Equal,
                        left: value,
                        right: expected,
                    });
                    self.terminate(MirTerminator::Branch {
                        condition,
                        then_target: arm_block,
                        else_target: next,
                    });
                    None
                }
                rsscript_syntax::ast::MatchPattern::Variant { name, bindings, .. } => {
                    if let Some(ok) = result_variant_tag(name) {
                        let binding = self.result_pattern_binding(bindings)?;
                        self.terminate(MirTerminator::MatchResult {
                            value,
                            ok_target: if ok { arm_block } else { next },
                            err_target: if ok { next } else { arm_block },
                        });
                        Some(MatchBindings::Result { ok, binding })
                    } else if let Some(some) = option_variant_tag(name) {
                        let binding = self.option_pattern_binding(some, bindings)?;
                        self.terminate(MirTerminator::MatchOption {
                            value,
                            some_target: if some { arm_block } else { next },
                            none_target: if some { next } else { arm_block },
                        });
                        Some(MatchBindings::Option { some, binding })
                    } else {
                        let layout = self.variant_pattern_layout(name, bindings)?;
                        self.terminate(MirTerminator::MatchVariant {
                            value,
                            expected: name.clone(),
                            match_target: arm_block,
                            else_target: next,
                        });
                        Some(MatchBindings::Variant(layout, bindings.clone()))
                    }
                }
                _ => return self.unsupported("non-literal checked HIR match expression pattern"),
            };

            self.current = arm_block;
            if let Some(bindings) = variant_bindings {
                self.lower_match_bindings(value, bindings)?;
            }
            self.lower_match_expression_arm(&arm.body, result_place)?;
            if self.current_block().terminator.is_none() {
                self.terminate(MirTerminator::Jump(join));
            }
            self.current = next;
        }
        self.terminate(MirTerminator::Unreachable);
        self.current = join;
        let destination = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination,
            place: result_place,
        });
        Ok(destination)
    }

    /// Resolve the checked semantic layout before emitting a match edge. The
    /// direct MIR subset deliberately accepts only a flat positional binding
    /// or wildcard for each declared field: nested patterns require their own
    /// projection and cleanup semantics.
    fn variant_pattern_layout(
        &self,
        name: &str,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<VariantLayout, MirLoweringError> {
        let Some(layout) = self.targets.variants.get(name) else {
            return self.unsupported("unresolved checked HIR variant match pattern");
        };
        if layout.fields.len() != bindings.len() {
            return self.unsupported("checked HIR variant match binding arity");
        }
        if bindings.iter().any(|binding| {
            !matches!(
                binding,
                rsscript_syntax::ast::MatchPattern::Binding { .. }
                    | rsscript_syntax::ast::MatchPattern::Wildcard(_)
            )
        }) {
            return self.unsupported("nested checked HIR variant match binding");
        }
        Ok(layout.clone())
    }

    fn lower_variant_pattern_bindings(
        &mut self,
        value: ValueId,
        layout: &VariantLayout,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<(), MirLoweringError> {
        for (field, binding) in layout.fields.iter().zip(bindings) {
            let rsscript_syntax::ast::MatchPattern::Binding { name, .. } = binding else {
                continue;
            };
            let destination = self.value();
            self.emit(MirInstruction::GetField {
                destination,
                base: value,
                field: field.clone(),
            });
            let place = self.place(name);
            self.emit(MirInstruction::WritePlace {
                place,
                value: destination,
            });
        }
        Ok(())
    }

    fn result_pattern_binding(
        &self,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<rsscript_syntax::ast::MatchPattern, MirLoweringError> {
        let [binding] = bindings else {
            return self.unsupported("checked HIR Result match binding arity");
        };
        if !matches!(
            binding,
            rsscript_syntax::ast::MatchPattern::Binding { .. }
                | rsscript_syntax::ast::MatchPattern::Wildcard(_)
        ) {
            return self.unsupported("nested checked HIR Result match binding");
        }
        Ok(binding.clone())
    }

    fn lower_match_bindings(
        &mut self,
        value: ValueId,
        bindings: MatchBindings,
    ) -> Result<(), MirLoweringError> {
        match bindings {
            MatchBindings::Variant(layout, bindings) => {
                self.lower_variant_pattern_bindings(value, &layout, &bindings)
            }
            MatchBindings::Result { ok, binding } => {
                let rsscript_syntax::ast::MatchPattern::Binding { name, .. } = binding else {
                    return Ok(());
                };
                let destination = self.value();
                self.emit(MirInstruction::UnwrapResult {
                    destination,
                    source: value,
                    ok,
                });
                let place = self.place(&name);
                self.emit(MirInstruction::WritePlace {
                    place,
                    value: destination,
                });
                Ok(())
            }
            MatchBindings::Option { some, binding } => {
                let Some(rsscript_syntax::ast::MatchPattern::Binding { name, .. }) = binding else {
                    return Ok(());
                };
                let destination = self.value();
                self.emit(MirInstruction::UnwrapOption {
                    destination,
                    source: value,
                });
                let place = self.place(&name);
                self.emit(MirInstruction::WritePlace {
                    place,
                    value: destination,
                });
                debug_assert!(some, "only Some patterns bind an Option payload");
                Ok(())
            }
        }
    }

    fn option_pattern_binding(
        &self,
        some: bool,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<Option<rsscript_syntax::ast::MatchPattern>, MirLoweringError> {
        if !some {
            if bindings.is_empty() {
                return Ok(None);
            }
            return self.unsupported("checked HIR None match binding arity");
        }
        let [binding] = bindings else {
            return self.unsupported("checked HIR Some match binding arity");
        };
        if !matches!(
            binding,
            rsscript_syntax::ast::MatchPattern::Binding { .. }
                | rsscript_syntax::ast::MatchPattern::Wildcard(_)
        ) {
            return self.unsupported("nested checked HIR Some match binding");
        }
        Ok(Some(binding.clone()))
    }

    fn lower_match_expression_arm(
        &mut self,
        body: &checked::HirBlock,
        result_place: PlaceId,
    ) -> Result<(), MirLoweringError> {
        let Some((last, initial)) = body.statements.split_last() else {
            return self.unsupported("empty checked HIR match expression arm");
        };
        for statement in initial {
            self.lower_statement(statement)?;
            if self.current_block().terminator.is_some() {
                return self.unsupported("terminating statement before match expression value");
            }
        }
        match last {
            checked::HirStmt::Expr(expression) => {
                let value = self.lower_expression(expression)?;
                self.emit(MirInstruction::WritePlace {
                    place: result_place,
                    value,
                });
                Ok(())
            }
            checked::HirStmt::Return { .. } => self.lower_statement(last),
            _ => self.unsupported("checked HIR match expression arm without value"),
        }
    }

    fn lower_loop(
        &mut self,
        condition: Option<&checked::HirExpr>,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        let header = self.new_block();
        let body_block = self.new_block();
        let exit = self.new_block();
        self.terminate(MirTerminator::Jump(header));

        self.current = header;
        if let Some(condition) = condition {
            let condition = self.lower_expression(condition)?;
            self.terminate(MirTerminator::Branch {
                condition,
                then_target: body_block,
                else_target: exit,
            });
        } else {
            self.terminate(MirTerminator::Jump(body_block));
        }

        self.current = body_block;
        self.loops.push(LoopTargets {
            continue_target: header,
            break_target: exit,
            cleanup_depth: self.resource_scopes.len(),
        });
        self.lower_checked_block(body)?;
        self.loops.pop();
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(header));
        }
        self.current = exit;
        Ok(())
    }

    /// Lower synchronous `for item in List<T>` into explicit index-based CFG.
    /// Non-list and async iterator protocols remain fail-closed until MIR owns
    /// their runtime and cancellation semantics.
    fn lower_for(
        &mut self,
        binding: &str,
        iterable: &checked::HirExpr,
        iterable_type: Option<&rsscript_semantics::ResolvedType>,
        is_async: bool,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        if is_async {
            return self.unsupported("async checked HIR for loop");
        }
        let Some(iterable_type) = iterable_type else {
            return self.unsupported("checked HIR for loop without resolved iterable type");
        };
        if iterable_type.root_name() != Some("List") || iterable_type.arguments().len() != 1 {
            return self.unsupported("non-list checked HIR for loop");
        }

        let list = self.lower_expression(iterable)?;
        let index_place = self.place(&format!("$for_index_{}", self.place_names.len()));
        let zero = self.literal(MirLiteral::Int(0))?;
        self.emit(MirInstruction::WritePlace {
            place: index_place,
            value: zero,
        });
        let one = self.literal(MirLiteral::Int(1))?;
        let length = self.value();
        self.emit(MirInstruction::ListLen {
            destination: length,
            list,
        });

        let header = self.new_block();
        let body_block = self.new_block();
        let increment = self.new_block();
        let exit = self.new_block();
        self.terminate(MirTerminator::Jump(header));

        self.current = header;
        let index = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination: index,
            place: index_place,
        });
        let in_bounds = self.value();
        self.emit(MirInstruction::Binary {
            destination: in_bounds,
            op: MirBinaryOp::Less,
            left: index,
            right: length,
        });
        self.terminate(MirTerminator::Branch {
            condition: in_bounds,
            then_target: body_block,
            else_target: exit,
        });

        self.current = body_block;
        let item = self.value();
        self.emit(MirInstruction::ListGet {
            destination: item,
            list,
            index,
        });
        let binding = self.place(binding);
        self.emit(MirInstruction::WritePlace {
            place: binding,
            value: item,
        });
        self.loops.push(LoopTargets {
            continue_target: increment,
            break_target: exit,
            cleanup_depth: self.resource_scopes.len(),
        });
        self.lower_checked_block(body)?;
        self.loops.pop();
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(increment));
        }

        self.current = increment;
        let current = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination: current,
            place: index_place,
        });
        let next = self.value();
        self.emit(MirInstruction::Binary {
            destination: next,
            op: MirBinaryOp::Add,
            left: current,
            right: one,
        });
        self.emit(MirInstruction::WritePlace {
            place: index_place,
            value: next,
        });
        self.terminate(MirTerminator::Jump(header));
        self.current = exit;
        Ok(())
    }

    fn lower_with(
        &mut self,
        resource: &checked::HirExpr,
        resource_type: Option<&rsscript_semantics::ResolvedType>,
        binding: &str,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        let Some(resource_type) = resource_type else {
            return self.unsupported("checked HIR resource scope without structural type");
        };
        let wire = checked_type_to_wire(resource_type, &self.function_name)?;
        let Some(resource_type) = self.intern_resource_wire_type(wire) else {
            return self.unsupported("checked HIR resource scope is not a resource type");
        };
        let source_expression = match resource {
            checked::HirExpr::Manage { value, .. } => value.as_ref(),
            other => other,
        };
        let source = self.lower_expression(source_expression)?;
        let place = self.place(binding);
        self.emit(MirInstruction::AcquireResource {
            place,
            resource_type,
            source,
        });
        self.resource_scopes.push(place);
        self.lower_checked_block(body)?;
        if self.current_block().terminator.is_none() {
            self.emit(MirInstruction::ReleaseResource { place });
        }
        let released = self.resource_scopes.pop();
        debug_assert_eq!(released, Some(place));
        Ok(())
    }

    fn intern_resource_wire_type(&mut self, wire: WireType) -> Option<ResourceTypeId> {
        let name = resource_type_name_from_wire(&wire)?;
        Some(ResourceTypeId::new(
            self.types.intern(WireType::Resource { name }).index() as u32,
        ))
    }

    fn lower_checked_block(&mut self, block: &checked::HirBlock) -> Result<(), MirLoweringError> {
        for statement in &block.statements {
            if self.current_block().terminator.is_some() {
                return self.unsupported("statement after checked HIR return");
            }
            self.lower_statement(statement)?;
        }
        Ok(())
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId::new(self.blocks.len() as u32);
        self.blocks.push(BlockDraft::new());
        id
    }

    fn current_block(&self) -> &BlockDraft {
        &self.blocks[self.current.index()]
    }

    fn current_block_mut(&mut self) -> &mut BlockDraft {
        &mut self.blocks[self.current.index()]
    }

    fn emit(&mut self, instruction: MirInstruction) {
        self.current_block_mut().instructions.push(instruction);
    }

    fn terminate(&mut self, terminator: MirTerminator) {
        debug_assert!(self.current_block().terminator.is_none());
        self.current_block_mut().terminator = Some(terminator);
    }

    fn start_detached_block(&mut self) {
        self.current = self.new_block();
    }

    fn emit_resource_cleanup_from(&mut self, depth: usize) {
        let places = self.resource_cleanup_places_from(depth);
        for place in places {
            self.emit(MirInstruction::ReleaseResource { place });
        }
    }

    fn resource_cleanup_places(&self) -> Vec<PlaceId> {
        self.resource_cleanup_places_from(0)
    }

    fn resource_cleanup_places_from(&self, depth: usize) -> Vec<PlaceId> {
        self.resource_scopes[depth..]
            .iter()
            .rev()
            .copied()
            .collect()
    }

    fn place(&mut self, name: &str) -> PlaceId {
        if let Some(place) = self.places.get(name) {
            return *place;
        }
        let place = PlaceId::new(self.place_names.len() as u32);
        self.places.insert(name.to_owned(), place);
        self.place_names.push(name.to_owned());
        place
    }

    fn place_with_type(&mut self, name: &str, ty: TypeId) -> PlaceId {
        let place = self.place(name);
        self.place_types.insert(name.to_owned(), ty);
        place
    }

    fn place_type(&self, name: &str) -> Result<TypeId, MirLoweringError> {
        self.place_types
            .get(name)
            .copied()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "checked HIR closure capture without a resolved local type",
            })
    }

    fn lookup_place(&self, name: &str) -> Result<PlaceId, MirLoweringError> {
        self.places
            .get(name)
            .copied()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "unknown checked HIR local",
            })
    }

    fn value(&mut self) -> ValueId {
        let value = ValueId::new(self.next_value);
        self.next_value += 1;
        value
    }

    fn task(&mut self) -> TaskId {
        let task = TaskId::new(self.next_task);
        self.next_task += 1;
        task
    }

    fn unsupported<T>(&self, construct: &'static str) -> Result<T, MirLoweringError> {
        Err(MirLoweringError::Unsupported {
            function: self.function_name.to_owned(),
            construct,
        })
    }
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
