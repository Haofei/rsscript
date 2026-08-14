//! Transitional lowering from owned executable IR to typed CFG MIR.
//!
//! This bridge deliberately supports only the pure control-flow subset. It
//! gives the new backend boundary an executable-independent, verified model
//! without pretending that resources, structured async, or external calls have
//! already migrated. Unsupported nodes fail closed and stay on the legacy path.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

use rsscript_abi_model::{
    DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature, WireType,
};
use rsscript_exec_ir::{
    BinaryOp, Callee, ExecutableExpr, ExecutableFunction, ExecutableIr, ExecutableStmt, ParamEffect,
};
use rsscript_mir::{
    BasicBlock, BlockId, FunctionId, MirBinaryOp, MirCallArgument, MirCallTarget,
    MirExternalImport, MirFunction, MirFunctionDebug, MirFunctionSignature, MirInstruction,
    MirLiteral, MirModule, MirParameterMode, MirTerminator, PlaceId, ResourceTypeId, TaskGroupId,
    TaskId, TypeId, ValueId,
};
use rsscript_semantics::hir as checked;

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

/// Lower the currently supported pure subset of owned executable IR into typed
/// CFG MIR. This is intentionally a transitional entry point; the final path
/// will consume checked HIR directly once all semantic facts are owned by the
/// semantics query boundary.
pub fn lower_executable_ir_to_mir(
    executable: &ExecutableIr,
) -> Result<MirModule, MirLoweringError> {
    let functions = executable.functions().collect::<Vec<_>>();
    let mut types = TypeTable::default();
    let signatures = functions
        .iter()
        .map(|function| types.function_signature(&function.signature))
        .collect::<Vec<_>>();
    let targets = CallTargets {
        functions: functions
            .iter()
            .enumerate()
            .map(|(index, function)| (function.name.clone(), FunctionId::new(index as u32)))
            .collect(),
        external_imports: executable
            .external_imports()
            .iter()
            .enumerate()
            .map(|(index, import)| {
                (
                    import.symbol.as_str().to_owned(),
                    rsscript_mir::ExternalSymbolId::new(index as u32),
                )
            })
            .collect(),
        variants: BTreeMap::new(),
    };
    let mut lowered = Vec::with_capacity(functions.len());
    let mut debug = Vec::with_capacity(functions.len());
    for ((index, function), signature) in functions.iter().enumerate().zip(signatures) {
        let output = FunctionLowerer::new(
            FunctionId::new(index as u32),
            function,
            signature,
            targets.clone(),
            &mut types,
        )
        .lower()?;
        lowered.push(output.function);
        debug.push(output.debug);
    }
    let imports = executable
        .external_imports()
        .iter()
        .enumerate()
        .map(|(index, import)| {
            MirExternalImport::new(
                rsscript_mir::ExternalSymbolId::new(index as u32),
                import.symbol.clone(),
                import.signature.clone(),
            )
        })
        .collect();
    Ok(MirModule::new(types.into_types(), lowered, debug, imports)?)
}

/// Lower the deliberately small checked-HIR subset without projecting
/// through `ExecutableIr`. This is the first replacement path for the
/// transitional source-shaped bridge: it consumes checked HIR nodes and their
/// type facts directly. Unsupported resources and async constructs continue to
/// fail closed so callers can choose the explicit compatibility path during
/// migration.
pub fn lower_checked_hir_to_mir(hir: &checked::Hir) -> Result<MirModule, MirLoweringError> {
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
        .map(|(_, _, signature)| types.checked_function_signature(signature))
        .collect::<Vec<_>>();
    let external_imports = checked_external_imports(hir)?;
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
    let targets = CallTargets {
        functions: functions
            .iter()
            .enumerate()
            .map(|(index, (name, _, _))| (name.to_string(), FunctionId::new(index as u32)))
            .collect(),
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
        variants,
    };
    let mut lowered = Vec::with_capacity(functions.len());
    let mut debug = Vec::with_capacity(functions.len());
    for ((index, (name, block, signature)), mir_signature) in
        functions.iter().enumerate().zip(signatures)
    {
        let output = CheckedHirLowerer::new(
            FunctionId::new(index as u32),
            name,
            block,
            signature,
            mir_signature,
            targets.clone(),
            &mut types,
        )
        .lower()?;
        lowered.push(output.function);
        debug.push(output.debug);
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
    Ok(MirModule::new(types.into_types(), lowered, debug, imports)?)
}

fn checked_external_imports(
    hir: &checked::Hir,
) -> Result<Vec<(ExternalSymbol, FunctionSignature)>, MirLoweringError> {
    let mut imports = BTreeMap::new();
    for call in hir.call_sites() {
        let checked::CallResolution::Resolved { signature, .. } = &call.resolution else {
            continue;
        };
        if !signature.is_external {
            continue;
        }
        let symbol = checked_external_symbol(signature)?;
        imports
            .entry(symbol.as_str().to_owned())
            .or_insert((symbol, checked_external_signature(signature)));
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

fn checked_external_signature(signature: &checked::FunctionSig) -> FunctionSignature {
    FunctionSignature {
        parameters: signature
            .params
            .iter()
            .map(|parameter| ParameterSignature {
                name: parameter.name.clone(),
                effect: match parameter.effect.unwrap_or(checked::ParamEffect::Read) {
                    checked::ParamEffect::Read => DataEffect::Read,
                    checked::ParamEffect::Mut => DataEffect::Mut,
                    checked::ParamEffect::Take => DataEffect::Take,
                },
                ty: WireType::parse(&parameter.ty.to_string()),
                retained: signature.retained_params.contains(&parameter.name),
            })
            .collect(),
        result: signature
            .return_ty
            .as_ref()
            .map(|ty| WireType::parse(&ty.to_string()))
            .unwrap_or(WireType::Unit),
        asynchronous: signature.is_async,
    }
}

#[derive(Clone)]
struct CallTargets {
    functions: BTreeMap<String, FunctionId>,
    external_imports: BTreeMap<String, rsscript_mir::ExternalSymbolId>,
    variants: BTreeMap<String, VariantLayout>,
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
    fn function_signature(
        &mut self,
        signature: &rsscript_exec_ir::ExecutableSignature,
    ) -> MirFunctionSignature {
        MirFunctionSignature::with_modes(
            signature
                .params
                .iter()
                .map(|parameter| self.intern(WireType::parse(&parameter.type_name)))
                .collect(),
            signature
                .params
                .iter()
                .map(
                    |parameter| match parameter.effect.unwrap_or(ParamEffect::Read) {
                        ParamEffect::Read => MirParameterMode::Read,
                        ParamEffect::Mut => MirParameterMode::Mut,
                        ParamEffect::Take => MirParameterMode::Take,
                    },
                )
                .collect(),
            self.intern(
                signature
                    .return_type
                    .as_deref()
                    .map(WireType::parse)
                    .unwrap_or(WireType::Unit),
            ),
            signature.is_async,
        )
    }

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
        signature: &checked::FunctionSig,
    ) -> MirFunctionSignature {
        MirFunctionSignature::with_modes(
            signature
                .params
                .iter()
                .map(|parameter| self.intern(WireType::parse(&parameter.ty.to_string())))
                .collect(),
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
                .collect(),
            self.intern(
                signature
                    .return_ty
                    .as_ref()
                    .map(|ty| WireType::parse(&ty.to_string()))
                    .unwrap_or(WireType::Unit),
            ),
            signature.is_async,
        )
    }

    fn into_types(self) -> Vec<WireType> {
        self.types
    }
}

struct LoweredFunction {
    function: MirFunction,
    debug: MirFunctionDebug,
}

/// Direct checked-HIR lowering for the initial subset. Keeping this
/// separate from `FunctionLowerer` makes the temporary compatibility boundary
/// auditable: this implementation never constructs or reads `ExecutableIr`.
struct CheckedHirLowerer<'source, 'types> {
    id: FunctionId,
    function_name: &'source str,
    body: &'source checked::HirBlock,
    signature: &'source checked::FunctionSig,
    mir_signature: MirFunctionSignature,
    targets: CallTargets,
    types: &'types mut TypeTable,
    blocks: Vec<BlockDraft>,
    current: BlockId,
    places: HashMap<String, PlaceId>,
    place_names: Vec<String>,
    next_value: u32,
    tasks: HashMap<String, TaskId>,
    next_task: u32,
    loops: Vec<LoopTargets>,
    resource_scopes: Vec<PlaceId>,
}

impl<'source, 'types> CheckedHirLowerer<'source, 'types> {
    fn new(
        id: FunctionId,
        function_name: &'source str,
        body: &'source checked::HirBlock,
        signature: &'source checked::FunctionSig,
        mir_signature: MirFunctionSignature,
        targets: CallTargets,
        types: &'types mut TypeTable,
    ) -> Self {
        let mut lowerer = Self {
            id,
            function_name,
            body,
            signature,
            mir_signature,
            targets,
            types,
            blocks: vec![BlockDraft::new()],
            current: BlockId::new(0),
            places: HashMap::new(),
            place_names: Vec::new(),
            next_value: 0,
            tasks: HashMap::new(),
            next_task: 0,
            loops: Vec::new(),
            resource_scopes: Vec::new(),
        };
        for parameter in &lowerer.signature.params {
            lowerer.place(&parameter.name);
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
            function: MirFunction::new(
                self.id,
                self.mir_signature,
                self.place_names.len() as u32,
                self.next_value,
                blocks,
            ),
            debug: MirFunctionDebug::new(self.function_name.to_owned(), self.place_names),
        })
    }

    fn lower_statement(&mut self, statement: &checked::HirStmt) -> Result<(), MirLoweringError> {
        match statement {
            checked::HirStmt::Let {
                name,
                value,
                is_async: false,
                ..
            } => {
                let place = self.place(name);
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
                let checked::HirExpr::Ident { name, .. } = target else {
                    return self.unsupported("non-local checked HIR assignment");
                };
                let place = self.lookup_place(name)?;
                let value = self.lower_expression(value)?;
                self.emit(MirInstruction::WritePlace { place, value });
                Ok(())
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
                binding,
                body,
                ..
            } => self.lower_with(resource, binding, body),
            checked::HirStmt::For {
                binding,
                iterable,
                iterable_type_name,
                is_async,
                body,
                ..
            } => self.lower_for(
                binding,
                iterable,
                iterable_type_name.as_deref(),
                *is_async,
                body,
            ),
            checked::HirStmt::Match { value, arms, .. } => self.lower_match(value, arms),
            checked::HirStmt::Select { .. } => self.unsupported("checked HIR select"),
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

    fn lower_expression(
        &mut self,
        expression: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        match expression {
            checked::HirExpr::Ident { name, .. } if name == "Unit" => {
                self.literal(MirLiteral::Unit)
            }
            checked::HirExpr::Ident { name, .. } if name == "true" => {
                self.literal(MirLiteral::Bool(true))
            }
            checked::HirExpr::Ident { name, .. } if name == "false" => {
                self.literal(MirLiteral::Bool(false))
            }
            checked::HirExpr::Ident { name, .. } => {
                let destination = self.value();
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::ReadPlace { destination, place });
                Ok(destination)
            }
            checked::HirExpr::Number { value, .. } => {
                let value = value
                    .parse::<i64>()
                    .map(MirLiteral::Int)
                    .or_else(|_| value.parse::<f64>().map(MirLiteral::Float))
                    .map_err(|_| MirLoweringError::Unsupported {
                        function: self.function_name.to_owned(),
                        construct: "non-numeric checked HIR literal",
                    })?;
                self.literal(value)
            }
            checked::HirExpr::String { value, .. } => {
                self.literal(MirLiteral::String(value.clone()))
            }
            checked::HirExpr::Char { value, .. } => {
                let mut chars = value.chars();
                match (chars.next(), chars.next()) {
                    (Some(value), None) => self.literal(MirLiteral::Char(value)),
                    _ => self.unsupported("invalid checked HIR char literal"),
                }
            }
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
            checked::HirExpr::Index { base, index, .. }
                if checked_hir_expression_type_name(base).is_some_and(is_list_type) =>
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
            checked::HirExpr::Index { .. } => self.unsupported("non-list checked HIR index"),
            checked::HirExpr::Call {
                callee,
                receiver,
                args,
                resolution,
                ..
            } => self.lower_direct_call(callee, receiver.as_ref(), args, resolution),
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
            checked::HirExpr::Manage { .. } => self.unsupported("checked HIR managed value"),
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
            checked::HirExpr::Closure { .. } => self.unsupported("checked HIR closure"),
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

    fn literal(&mut self, value: MirLiteral) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        self.emit(MirInstruction::LoadLiteral { destination, value });
        Ok(destination)
    }

    fn lower_direct_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        receiver: Option<&checked::HirCallReceiver>,
        args: &[checked::HirCallArg],
        resolution: &checked::CallResolution,
    ) -> Result<ValueId, MirLoweringError> {
        if receiver.is_some() {
            return self.unsupported("checked HIR receiver call");
        }
        if matches!(resolution, checked::CallResolution::EnumVariant) {
            return self.lower_enum_variant_call(callee, args);
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
        if signature.is_builtin {
            return self.lower_builtin_call(signature, args);
        }
        let target = if signature.is_external {
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
            self.targets
                .functions
                .get(&signature.name)
                .or_else(|| {
                    qualified
                        .as_ref()
                        .and_then(|name| self.targets.functions.get(name))
                })
                .copied()
                .map(MirCallTarget::Function)
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "direct checked HIR call target",
                })?
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let mut arguments = Vec::with_capacity(ordered.len());
        let mut retained_places = Vec::new();
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
            .map(|ty| WireType::parse(&ty.to_string()))
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
        signature: &checked::FunctionSig,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        match signature.name.as_str() {
            "Ok" | "Err" => {
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
                if !args.is_empty() {
                    return self.unsupported("Option None constructor with non-zero arity");
                }
                self.emit(MirInstruction::MakeOption {
                    destination,
                    value: None,
                });
            }
            _ => return self.unsupported("builtin checked HIR call"),
        }
        Ok(destination)
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
        if *effect == checked::ParamEffect::Read
            && !matches!(
                value.as_ref(),
                checked::HirExpr::Ident { .. } | checked::HirExpr::Manage { .. }
            )
        {
            return self.lower_expression(value).map(MirCallArgument::Value);
        }
        let value = match value.as_ref() {
            checked::HirExpr::Ident { .. } => value.as_ref(),
            checked::HirExpr::Manage { value, .. } => value.as_ref(),
            _ => return self.unsupported("checked HIR data effect on non-local value"),
        };
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("checked HIR data effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        Ok(match effect {
            checked::ParamEffect::Read => MirCallArgument::BorrowRead(place),
            checked::ParamEffect::Mut => MirCallArgument::BorrowMut(place),
            checked::ParamEffect::Take => MirCallArgument::Take(place),
        })
    }

    fn lower_async_binding(
        &mut self,
        name: &str,
        value: Option<&checked::HirExpr>,
    ) -> Result<(), MirLoweringError> {
        let Some(checked::HirExpr::Call {
            receiver,
            args,
            resolution,
            ..
        }) = value
        else {
            return self.unsupported("async checked HIR binding without direct call");
        };
        if receiver.is_some() {
            return self.unsupported("async checked HIR receiver call");
        }
        let checked::CallResolution::Resolved { signature, .. } = resolution else {
            return self.unsupported("unresolved async checked HIR call");
        };
        if signature.is_external || signature.is_builtin {
            return self.unsupported("async external or builtin checked HIR call");
        }
        let qualified = signature
            .namespace
            .as_ref()
            .map(|namespace| format!("{namespace}.{}", signature.name));
        let target = self
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
                construct: "direct async checked HIR call target",
            })?;
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_direct_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let task = self.task();
        if self.tasks.insert(name.to_owned(), task).is_some() {
            return self.unsupported("duplicate async checked HIR binding");
        }
        self.emit(MirInstruction::Spawn {
            task,
            group: TaskGroupId::new(0),
            target,
            arguments,
        });
        Ok(())
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
            resolution,
            ..
        } = value
        {
            let checked::CallResolution::Resolved { signature, .. } = resolution else {
                return self.unsupported("unresolved awaited checked HIR call");
            };
            if signature.is_external && signature.is_async {
                return self.lower_direct_call(callee, receiver.as_ref(), args, resolution);
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
                    let literal = match_literal(literal, self.function_name)?;
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
                    let expected = self.literal(match_literal(literal, self.function_name)?)?;
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
        iterable_type_name: Option<&str>,
        is_async: bool,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        if is_async {
            return self.unsupported("async checked HIR for loop");
        }
        let Some(iterable_type_name) = iterable_type_name else {
            return self.unsupported("checked HIR for loop without resolved iterable type");
        };
        if !iterable_type_name.trim_start().starts_with("List<") {
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
        binding: &str,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        let (source_expression, type_name) = match resource {
            checked::HirExpr::Manage {
                value, type_name, ..
            } => (value.as_ref(), type_name.as_deref()),
            other => (other, checked_hir_expression_type_name(other)),
        };
        let Some(type_name) = type_name else {
            return self.unsupported("checked HIR resource scope without canonical type");
        };
        let source = self.lower_expression(source_expression)?;
        let place = self.place(binding);
        let resource_type = self.intern_resource_type(type_name);
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

    fn intern_resource_type(&mut self, type_name: &str) -> ResourceTypeId {
        let wire = match WireType::parse(type_name) {
            WireType::Resource { name } => WireType::Resource { name },
            WireType::Qualified { value, .. }
                if matches!(value.as_ref(), WireType::Resource { .. }) =>
            {
                *value
            }
            _ => WireType::Resource {
                name: type_name.to_owned(),
            },
        };
        ResourceTypeId::new(self.types.intern(wire).index() as u32)
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

struct FunctionLowerer<'source, 'types> {
    id: FunctionId,
    source: &'source ExecutableFunction,
    signature: MirFunctionSignature,
    targets: CallTargets,
    types: &'types mut TypeTable,
    blocks: Vec<BlockDraft>,
    current: BlockId,
    places: HashMap<String, PlaceId>,
    place_names: Vec<String>,
    next_value: u32,
    tasks: HashMap<String, TaskId>,
    next_task: u32,
    loops: Vec<LoopTargets>,
    resource_scopes: Vec<PlaceId>,
}

impl<'source, 'types> FunctionLowerer<'source, 'types> {
    fn new(
        id: FunctionId,
        source: &'source ExecutableFunction,
        signature: MirFunctionSignature,
        targets: CallTargets,
        types: &'types mut TypeTable,
    ) -> Self {
        let mut lowerer = Self {
            id,
            source,
            signature,
            targets,
            types,
            blocks: vec![BlockDraft::new()],
            current: BlockId::new(0),
            places: HashMap::new(),
            place_names: Vec::new(),
            next_value: 0,
            tasks: HashMap::new(),
            next_task: 0,
            loops: Vec::new(),
            resource_scopes: Vec::new(),
        };
        for parameter in &source.signature.params {
            lowerer.place(&parameter.name);
        }
        lowerer
    }

    fn lower(mut self) -> Result<LoweredFunction, MirLoweringError> {
        self.lower_statements(&self.source.body.statements)?;
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
            function: MirFunction::new(
                self.id,
                self.signature,
                self.place_names.len() as u32,
                self.next_value,
                blocks,
            ),
            debug: MirFunctionDebug::new(self.source.name.clone(), self.place_names),
        })
    }

    fn lower_statements(&mut self, statements: &[ExecutableStmt]) -> Result<(), MirLoweringError> {
        for statement in statements {
            self.lower_statement(statement)?;
        }
        Ok(())
    }

    fn lower_statement(&mut self, statement: &ExecutableStmt) -> Result<(), MirLoweringError> {
        match statement {
            ExecutableStmt::Let {
                name,
                value,
                is_async,
            } => {
                if *is_async {
                    return self.lower_async_binding(name, value.as_ref());
                }
                let place = self.place(name);
                if let Some(value) = value {
                    let value = self.lower_expression(value)?;
                    self.emit(MirInstruction::WritePlace { place, value });
                }
            }
            ExecutableStmt::Return { value } => {
                let value = value
                    .as_ref()
                    .map(|value| self.lower_expression(value))
                    .transpose()?;
                self.emit_resource_cleanup_from(0);
                self.terminate(MirTerminator::Return(value));
                self.start_detached_block();
            }
            ExecutableStmt::If {
                condition,
                then_body,
                else_body,
            } => self.lower_if(condition, then_body, else_body.as_ref())?,
            ExecutableStmt::Loop { condition, body } => {
                self.lower_loop(condition.as_ref(), body)?
            }
            ExecutableStmt::Assign { target, value } => {
                let ExecutableExpr::Ident { name, .. } = target else {
                    return self.unsupported("non-local assignment");
                };
                let place = self.lookup_place(name)?;
                let value = self.lower_expression(value)?;
                self.emit(MirInstruction::WritePlace { place, value });
            }
            ExecutableStmt::Break => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("break outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.break_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
            }
            ExecutableStmt::Continue => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("continue outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.continue_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
            }
            ExecutableStmt::Expr(expression) => {
                let value = self.lower_expression(expression)?;
                self.emit(MirInstruction::Discard { value });
            }
            ExecutableStmt::With {
                resource,
                binding,
                body,
            } => self.lower_with(resource, binding, body)?,
            ExecutableStmt::For { .. } => return self.unsupported("for loop"),
            ExecutableStmt::Match { .. } => return self.unsupported("match"),
            ExecutableStmt::Select { .. } => return self.unsupported("select"),
            ExecutableStmt::Unknown => return self.unsupported("unknown statement"),
        }
        Ok(())
    }

    fn lower_if(
        &mut self,
        condition: &ExecutableExpr,
        then_body: &rsscript_exec_ir::ExecutableBlock,
        else_body: Option<&rsscript_exec_ir::ExecutableBlock>,
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
        self.lower_statements(&then_body.statements)?;
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = else_block;
        if let Some(else_body) = else_body {
            self.lower_statements(&else_body.statements)?;
        }
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = join_block;
        Ok(())
    }

    fn lower_with(
        &mut self,
        resource: &ExecutableExpr,
        binding: &str,
        body: &rsscript_exec_ir::ExecutableBlock,
    ) -> Result<(), MirLoweringError> {
        let ExecutableExpr::Manage {
            value, type_name, ..
        } = resource
        else {
            return self.unsupported("unmanaged resource scope");
        };
        let Some(type_name) = type_name.as_deref() else {
            return self.unsupported("resource scope without canonical type");
        };
        // Evaluate the managed source before entering the lifetime. The current
        // MIR acquire primitive represents ownership of the resulting host
        // resource; bytecode execution remains intentionally unsupported.
        let source = self.lower_expression(value)?;
        let place = self.place(binding);
        let resource_type = self.intern_resource_type(type_name);
        self.emit(MirInstruction::AcquireResource {
            place,
            resource_type,
            source,
        });
        self.resource_scopes.push(place);
        self.lower_statements(&body.statements)?;
        if self.current_block().terminator.is_none() {
            self.emit(MirInstruction::ReleaseResource { place });
        }
        let released = self.resource_scopes.pop();
        debug_assert_eq!(released, Some(place));
        Ok(())
    }

    fn intern_resource_type(&mut self, type_name: &str) -> ResourceTypeId {
        let wire = match WireType::parse(type_name) {
            WireType::Resource { name } => WireType::Resource { name },
            WireType::Qualified { value, .. }
                if matches!(value.as_ref(), WireType::Resource { .. }) =>
            {
                *value
            }
            _ => WireType::Resource {
                name: type_name.to_owned(),
            },
        };
        ResourceTypeId::new(self.types.intern(wire).index() as u32)
    }

    fn lower_loop(
        &mut self,
        condition: Option<&ExecutableExpr>,
        body: &rsscript_exec_ir::ExecutableBlock,
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
        self.lower_statements(&body.statements)?;
        self.loops.pop();
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(header));
        }
        self.current = exit;
        Ok(())
    }

    fn lower_expression(
        &mut self,
        expression: &ExecutableExpr,
    ) -> Result<ValueId, MirLoweringError> {
        match expression {
            ExecutableExpr::Ident { name, .. } => {
                let destination = self.value();
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::ReadPlace { destination, place });
                Ok(destination)
            }
            ExecutableExpr::Number { value } => {
                let literal = value
                    .parse::<i64>()
                    .map(MirLiteral::Int)
                    .or_else(|_| value.parse::<f64>().map(MirLiteral::Float))
                    .map_err(|_| MirLoweringError::Unsupported {
                        function: self.source.name.clone(),
                        construct: "non-numeric literal",
                    })?;
                let destination = self.value();
                self.emit(MirInstruction::LoadLiteral {
                    destination,
                    value: literal,
                });
                Ok(destination)
            }
            ExecutableExpr::String { value } => self.literal(MirLiteral::String(value.clone())),
            ExecutableExpr::Char { value } => {
                let mut chars = value.chars();
                let Some(character) = chars.next() else {
                    return self.unsupported("empty char literal");
                };
                if chars.next().is_some() {
                    return self.unsupported("multi-character char literal");
                }
                self.literal(MirLiteral::Char(character))
            }
            ExecutableExpr::Binary { op, left, right } => {
                let left = self.lower_expression(left)?;
                let right = self.lower_expression(right)?;
                let destination = self.value();
                self.emit(MirInstruction::Binary {
                    destination,
                    op: binary_op(*op),
                    left,
                    right,
                });
                Ok(destination)
            }
            ExecutableExpr::ObjectLiteral { fields, .. } => {
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
            ExecutableExpr::MapLiteral { entries, .. } => {
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
            ExecutableExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_expression(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeList { destination, items });
                Ok(destination)
            }
            ExecutableExpr::Field { base, name, .. } => {
                let base = self.lower_expression(base)?;
                let destination = self.value();
                self.emit(MirInstruction::GetField {
                    destination,
                    base,
                    field: name.clone(),
                });
                Ok(destination)
            }
            ExecutableExpr::Index { base, index } => {
                if !expression_type_name(base).is_some_and(is_list_type) {
                    return self.unsupported("non-list index access");
                }
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
            ExecutableExpr::Call {
                callee,
                receiver,
                args,
                ..
            } => self.lower_call(callee, receiver.is_some(), args),
            ExecutableExpr::Effect { effect, value, .. } => match effect {
                ParamEffect::Read => self.lower_read_borrow(value),
                ParamEffect::Mut => self.unsupported("mutable borrow"),
                ParamEffect::Take => self.lower_take(value),
            },
            ExecutableExpr::Manage { .. } => self.unsupported("managed resource"),
            ExecutableExpr::Spawn { .. } => self.unsupported("standalone spawn"),
            ExecutableExpr::Await { value, .. } => self.lower_await(value),
            ExecutableExpr::Try { .. } => self.unsupported("try"),
            ExecutableExpr::Closure { .. } => self.unsupported("closure"),
            ExecutableExpr::Match { .. } => self.unsupported("match expression"),
            ExecutableExpr::Unknown => self.unsupported("unknown expression"),
        }
    }

    fn literal(&mut self, value: MirLiteral) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        self.emit(MirInstruction::LoadLiteral { destination, value });
        Ok(destination)
    }

    fn lower_call(
        &mut self,
        callee: &Callee,
        has_receiver: bool,
        args: &[rsscript_exec_ir::ExecutableCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        if has_receiver {
            return self.unsupported("receiver call");
        }
        let name = match callee {
            Callee::Name(name) => name.clone(),
            Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
            Callee::ReceiverCall { .. } => return self.unsupported("receiver call"),
        };
        let target = self
            .targets
            .functions
            .get(&name)
            .copied()
            .map(MirCallTarget::Function)
            .or_else(|| {
                self.targets
                    .external_imports
                    .get(&name)
                    .copied()
                    .map(MirCallTarget::External)
            })
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.source.name.clone(),
                construct: "unresolved direct call",
            })?;
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let destination = self.value();
        self.emit(MirInstruction::Call {
            destination,
            target,
            arguments,
        });
        Ok(destination)
    }

    fn lower_async_binding(
        &mut self,
        name: &str,
        value: Option<&ExecutableExpr>,
    ) -> Result<(), MirLoweringError> {
        let Some(ExecutableExpr::Call {
            callee,
            receiver,
            args,
            ..
        }) = value
        else {
            return self.unsupported("async binding without direct call");
        };
        if receiver.is_some() {
            return self.unsupported("async receiver call");
        }
        let name_key = match callee {
            Callee::Name(name) => name.clone(),
            Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
            Callee::ReceiverCall { .. } => return self.unsupported("async receiver call"),
        };
        let Some(target) = self.targets.functions.get(&name_key).copied() else {
            return self.unsupported("async external or unresolved call");
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let task = self.task();
        if self.tasks.insert(name.to_owned(), task).is_some() {
            return self.unsupported("duplicate async binding");
        }
        self.emit(MirInstruction::Spawn {
            task,
            group: TaskGroupId::new(0),
            target,
            arguments,
        });
        Ok(())
    }

    fn lower_await(&mut self, value: &ExecutableExpr) -> Result<ValueId, MirLoweringError> {
        let ExecutableExpr::Ident { name, .. } = value else {
            return self.unsupported("await of non-task local");
        };
        let Some(task) = self.tasks.get(name).copied() else {
            return self.unsupported("await of unknown task");
        };
        let destination = self.value();
        self.emit(MirInstruction::Await { destination, task });
        Ok(destination)
    }

    fn lower_call_argument(
        &mut self,
        argument: &ExecutableExpr,
    ) -> Result<MirCallArgument, MirLoweringError> {
        let ExecutableExpr::Effect { effect, value, .. } = argument else {
            return self.lower_expression(argument).map(MirCallArgument::Value);
        };
        let ExecutableExpr::Ident { name, .. } = value.as_ref() else {
            return self.unsupported("data effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        Ok(match effect {
            ParamEffect::Read => MirCallArgument::BorrowRead(place),
            ParamEffect::Mut => MirCallArgument::BorrowMut(place),
            ParamEffect::Take => MirCallArgument::Take(place),
        })
    }

    fn lower_read_borrow(&mut self, value: &ExecutableExpr) -> Result<ValueId, MirLoweringError> {
        let ExecutableExpr::Ident { name, .. } = value else {
            return self.unsupported("read borrow of non-local value");
        };
        let destination = self.value();
        let place = self.lookup_place(name)?;
        self.emit(MirInstruction::BorrowRead { destination, place });
        Ok(destination)
    }

    fn lower_take(&mut self, value: &ExecutableExpr) -> Result<ValueId, MirLoweringError> {
        let ExecutableExpr::Ident { name, .. } = value else {
            return self.unsupported("move of non-local value");
        };
        let destination = self.value();
        let place = self.lookup_place(name)?;
        self.emit(MirInstruction::TakePlace { destination, place });
        Ok(destination)
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

    fn lookup_place(&self, name: &str) -> Result<PlaceId, MirLoweringError> {
        self.places
            .get(name)
            .copied()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.source.name.clone(),
                construct: "unresolved local",
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

    fn emit_resource_cleanup_from(&mut self, depth: usize) {
        let resources = self.resource_scopes[depth..]
            .iter()
            .copied()
            .rev()
            .collect::<Vec<_>>();
        for place in resources {
            self.emit(MirInstruction::ReleaseResource { place });
        }
    }

    fn emit(&mut self, instruction: MirInstruction) {
        self.current_block_mut().instructions.push(instruction);
    }

    fn terminate(&mut self, terminator: MirTerminator) {
        debug_assert!(self.current_block().terminator.is_none());
        self.current_block_mut().terminator = Some(terminator);
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId::new(self.blocks.len() as u32);
        self.blocks.push(BlockDraft::new());
        id
    }

    fn start_detached_block(&mut self) {
        self.current = self.new_block();
    }

    fn current_block(&self) -> &BlockDraft {
        &self.blocks[self.current.index()]
    }

    fn current_block_mut(&mut self) -> &mut BlockDraft {
        &mut self.blocks[self.current.index()]
    }

    fn unsupported<T>(&self, construct: &'static str) -> Result<T, MirLoweringError> {
        Err(MirLoweringError::Unsupported {
            function: self.source.name.clone(),
            construct,
        })
    }
}

fn expression_type_name(expression: &ExecutableExpr) -> Option<&str> {
    match expression {
        ExecutableExpr::Ident { type_name, .. }
        | ExecutableExpr::ObjectLiteral { type_name, .. }
        | ExecutableExpr::MapLiteral { type_name, .. }
        | ExecutableExpr::ArrayLiteral { type_name, .. }
        | ExecutableExpr::Call { type_name, .. }
        | ExecutableExpr::Effect { type_name, .. }
        | ExecutableExpr::Manage { type_name, .. }
        | ExecutableExpr::Spawn { type_name, .. }
        | ExecutableExpr::Await { type_name, .. }
        | ExecutableExpr::Try { type_name, .. }
        | ExecutableExpr::Match { type_name, .. } => type_name.as_deref(),
        ExecutableExpr::Number { .. }
        | ExecutableExpr::String { .. }
        | ExecutableExpr::Char { .. }
        | ExecutableExpr::Binary { .. }
        | ExecutableExpr::Field { .. }
        | ExecutableExpr::Index { .. }
        | ExecutableExpr::Closure { .. }
        | ExecutableExpr::Unknown => None,
    }
}

fn checked_hir_expression_type_name(expression: &checked::HirExpr) -> Option<&str> {
    match expression {
        checked::HirExpr::Ident { type_name, .. }
        | checked::HirExpr::ObjectLiteral { type_name, .. }
        | checked::HirExpr::MapLiteral { type_name, .. }
        | checked::HirExpr::ArrayLiteral { type_name, .. }
        | checked::HirExpr::Call { type_name, .. }
        | checked::HirExpr::Effect { type_name, .. }
        | checked::HirExpr::Manage { type_name, .. }
        | checked::HirExpr::Spawn { type_name, .. }
        | checked::HirExpr::Await { type_name, .. }
        | checked::HirExpr::Try { type_name, .. }
        | checked::HirExpr::Match { type_name, .. } => type_name.as_deref(),
        checked::HirExpr::Number { .. }
        | checked::HirExpr::String { .. }
        | checked::HirExpr::Char { .. }
        | checked::HirExpr::Binary { .. }
        | checked::HirExpr::Field { .. }
        | checked::HirExpr::Index { .. }
        | checked::HirExpr::Closure { .. }
        | checked::HirExpr::Unknown(_) => None,
    }
}

fn is_list_type(type_name: &str) -> bool {
    type_name == "List" || type_name.starts_with("List<")
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
        rsscript_syntax::ast::MatchLiteral::String(value) => Ok(MirLiteral::String(value.clone())),
        rsscript_syntax::ast::MatchLiteral::Char(value) => {
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(value), None) => Ok(MirLiteral::Char(value)),
                _ => Err(MirLoweringError::Unsupported {
                    function: function_name.to_owned(),
                    construct: "invalid checked HIR match char literal",
                }),
            }
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

fn binary_op(op: BinaryOp) -> MirBinaryOp {
    match op {
        BinaryOp::Add => MirBinaryOp::Add,
        BinaryOp::Subtract => MirBinaryOp::Subtract,
        BinaryOp::Multiply => MirBinaryOp::Multiply,
        BinaryOp::Divide => MirBinaryOp::Divide,
        BinaryOp::Modulo => MirBinaryOp::Modulo,
        BinaryOp::BitAnd => MirBinaryOp::BitAnd,
        BinaryOp::BitOr => MirBinaryOp::BitOr,
        BinaryOp::BitXor => MirBinaryOp::BitXor,
        BinaryOp::ShiftLeft => MirBinaryOp::ShiftLeft,
        BinaryOp::ShiftRight => MirBinaryOp::ShiftRight,
        BinaryOp::Equal => MirBinaryOp::Equal,
        BinaryOp::NotEqual => MirBinaryOp::NotEqual,
        BinaryOp::Less => MirBinaryOp::Less,
        BinaryOp::LessEqual => MirBinaryOp::LessEqual,
        BinaryOp::Greater => MirBinaryOp::Greater,
        BinaryOp::GreaterEqual => MirBinaryOp::GreaterEqual,
        BinaryOp::LogicalAnd => MirBinaryOp::LogicalAnd,
        BinaryOp::LogicalOr => MirBinaryOp::LogicalOr,
    }
}

#[cfg(test)]
mod tests {
    use rsscript_exec_ir::{
        ExecutableBlock, ExecutableFieldAccess, ExecutableFunction, ExecutableMapLiteralEntry,
        ExecutableObjectLiteralField, ExecutableProgram, ExecutableSignature,
    };

    use super::*;

    fn module(function: ExecutableFunction) -> ExecutableIr {
        module_with_functions(vec![function])
    }

    fn module_with_functions(functions: Vec<ExecutableFunction>) -> ExecutableIr {
        let mut program = ExecutableProgram::default();
        for function in functions {
            program.insert_function(function.name.clone(), function);
        }
        ExecutableIr::new(program, Box::new([]))
    }

    fn signature() -> ExecutableSignature {
        ExecutableSignature {
            namespace: None,
            name: "main".into(),
            is_async: false,
            params: Vec::new(),
            return_type: Some("Int".into()),
            is_external: false,
        }
    }

    #[test]
    fn lowers_scalar_branching_to_verified_cfg() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Let {
                        name: "value".into(),
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                        is_async: false,
                    },
                    ExecutableStmt::If {
                        condition: ExecutableExpr::Binary {
                            op: BinaryOp::Less,
                            left: Box::new(ExecutableExpr::Ident {
                                name: "value".into(),
                                type_name: Some("Int".into()),
                            }),
                            right: Box::new(ExecutableExpr::Number { value: "2".into() }),
                        },
                        then_body: ExecutableBlock {
                            statements: vec![ExecutableStmt::Return {
                                value: Some(ExecutableExpr::Ident {
                                    name: "value".into(),
                                    type_name: Some("Int".into()),
                                }),
                            }],
                        },
                        else_body: Some(ExecutableBlock {
                            statements: vec![ExecutableStmt::Return {
                                value: Some(ExecutableExpr::Number { value: "0".into() }),
                            }],
                        }),
                    },
                ],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).unwrap();
        assert_eq!(mir.functions().len(), 1);
        assert_eq!(
            mir.types(),
            &[WireType::Int {
                bits: 64,
                signed: true
            }]
        );
        assert_eq!(mir.functions()[0].signature().result(), TypeId::new(0));
        assert!(mir.functions()[0].blocks().len() >= 4);
        mir.verify().unwrap();
    }

    #[test]
    fn lowers_standalone_take_of_a_local_to_explicit_mir_move() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Let {
                        name: "value".into(),
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                        is_async: false,
                    },
                    ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Effect {
                            effect: ParamEffect::Take,
                            value: Box::new(ExecutableExpr::Ident {
                                name: "value".into(),
                                type_name: Some("Int".into()),
                            }),
                            type_name: Some("Int".into()),
                        }),
                    },
                ],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower standalone take");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::WritePlace { .. },
                MirInstruction::TakePlace { .. }
            ]
        ));
        mir.verify().expect("verify explicit move");
    }

    #[test]
    fn lowers_array_literals_to_owned_mir_list_construction() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::ArrayLiteral {
                        items: vec![
                            ExecutableExpr::Number { value: "1".into() },
                            ExecutableExpr::Number { value: "2".into() },
                        ],
                        type_name: Some("List<Int>".into()),
                    }),
                }],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower array literal");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::MakeList { items, .. },
            ] if items.len() == 2
        ));
        mir.verify().expect("verify list construction");
    }

    #[test]
    fn lowers_map_literals_to_owned_mir_map_construction() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::MapLiteral {
                        entries: vec![ExecutableMapLiteralEntry {
                            key: ExecutableExpr::Number { value: "1".into() },
                            value: ExecutableExpr::Number { value: "2".into() },
                        }],
                        type_name: Some("Map<Int, Int>".into()),
                    }),
                }],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower map literal");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::MakeMap { entries, .. },
            ] if entries.len() == 1
        ));
        mir.verify().expect("verify map construction");
    }

    #[test]
    fn lowers_json_object_literals_to_owned_mir_object_construction() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::ObjectLiteral {
                        fields: vec![ExecutableObjectLiteralField {
                            name: "count".into(),
                            value: ExecutableExpr::Number { value: "3".into() },
                        }],
                        type_name: Some("JsonValue".into()),
                    }),
                }],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower object literal");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::MakeObject { fields, .. },
            ] if fields == &[("count".into(), ValueId::new(0))]
        ));
        mir.verify().expect("verify object construction");
    }

    #[test]
    fn lowers_aggregate_field_reads_to_typed_mir() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::Field {
                        base: Box::new(ExecutableExpr::ObjectLiteral {
                            fields: vec![ExecutableObjectLiteralField {
                                name: "count".into(),
                                value: ExecutableExpr::Number { value: "3".into() },
                            }],
                            type_name: Some("JsonValue".into()),
                        }),
                        name: "count".into(),
                        access: ExecutableFieldAccess {
                            base_type: Some("JsonValue".into()),
                            type_name: Some("Int".into()),
                        },
                    }),
                }],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower aggregate field");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::MakeObject { .. },
                MirInstruction::GetField { field, .. },
            ] if field == "count"
        ));
        mir.verify().expect("verify aggregate field read");
    }

    #[test]
    fn lowers_checked_list_index_to_explicit_mir_list_get() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::Index {
                        base: Box::new(ExecutableExpr::ArrayLiteral {
                            items: vec![
                                ExecutableExpr::Number { value: "40".into() },
                                ExecutableExpr::Number { value: "2".into() },
                            ],
                            type_name: Some("List<Int>".into()),
                        }),
                        index: Box::new(ExecutableExpr::Number { value: "1".into() }),
                    }),
                }],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower list index");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::MakeList { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::ListGet { .. },
            ]
        ));
        mir.verify().expect("verify list get");
    }

    #[test]
    fn rejects_unmanaged_resource_and_async_nodes_until_their_mir_ops_exist() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::With {
                    resource: ExecutableExpr::Number { value: "1".into() },
                    binding: "resource".into(),
                    body: ExecutableBlock { statements: vec![] },
                }],
            },
        });

        assert!(matches!(
            lower_executable_ir_to_mir(&executable),
            Err(MirLoweringError::Unsupported {
                construct: "unmanaged resource scope",
                ..
            })
        ));
    }

    #[test]
    fn lowers_managed_resource_scope_cleanup_before_return() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::With {
                    resource: ExecutableExpr::Manage {
                        value: Box::new(ExecutableExpr::Call {
                            callee: Callee::Name("open".into()),
                            receiver: None,
                            args: Vec::new(),
                            type_name: Some("File".into()),
                        }),
                        type_name: Some("host.fs.File".into()),
                    },
                    binding: "file".into(),
                    body: ExecutableBlock {
                        statements: vec![ExecutableStmt::Return {
                            value: Some(ExecutableExpr::Number { value: "9".into() }),
                        }],
                    },
                }],
            },
        });
        let mut program = ExecutableProgram::default();
        program.insert_function(
            "open".into(),
            ExecutableFunction {
                name: "open".into(),
                is_async: false,
                signature: ExecutableSignature {
                    namespace: None,
                    name: "open".into(),
                    is_async: false,
                    params: Vec::new(),
                    return_type: Some("host.fs.File".into()),
                    is_external: false,
                },
                body: ExecutableBlock { statements: vec![] },
            },
        );
        program.insert_function(
            "main".into(),
            executable.functions().next().unwrap().clone(),
        );
        let mir = lower_executable_ir_to_mir(&ExecutableIr::new(program, Box::new([])))
            .expect("lower managed resource scope");
        let main = mir
            .functions()
            .iter()
            .find(|function| {
                mir.function_debug(function.id())
                    .is_some_and(|debug| debug.name() == "main")
            })
            .expect("main function");
        assert!(matches!(
            main.blocks()[0].instructions(),
            [
                MirInstruction::Call { .. },
                MirInstruction::AcquireResource { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::ReleaseResource { .. }
            ]
        ));
        mir.verify().expect("verify resource lifetime");
    }

    #[test]
    fn lowers_managed_resource_scope_cleanup_before_loop_break() {
        let main = ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Loop {
                        condition: None,
                        body: ExecutableBlock {
                            statements: vec![ExecutableStmt::With {
                                resource: ExecutableExpr::Manage {
                                    value: Box::new(ExecutableExpr::Call {
                                        callee: Callee::Name("open".into()),
                                        receiver: None,
                                        args: Vec::new(),
                                        type_name: Some("host.fs.File".into()),
                                    }),
                                    type_name: Some("host.fs.File".into()),
                                },
                                binding: "file".into(),
                                body: ExecutableBlock {
                                    statements: vec![ExecutableStmt::Break],
                                },
                            }],
                        },
                    },
                    ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                    },
                ],
            },
        };
        let open = ExecutableFunction {
            name: "open".into(),
            is_async: false,
            signature: ExecutableSignature {
                namespace: None,
                name: "open".into(),
                is_async: false,
                params: Vec::new(),
                return_type: Some("host.fs.File".into()),
                is_external: false,
            },
            body: ExecutableBlock { statements: vec![] },
        };
        let mir = lower_executable_ir_to_mir(&module_with_functions(vec![main, open]))
            .expect("lower resource scope in loop");
        mir.verify()
            .expect("break path releases the scoped resource before the loop exit");
    }

    #[test]
    fn lowers_async_binding_and_await_to_structured_task_ops() {
        let worker = ExecutableFunction {
            name: "worker".into(),
            is_async: true,
            signature: ExecutableSignature {
                namespace: None,
                name: "worker".into(),
                is_async: true,
                params: Vec::new(),
                return_type: Some("Int".into()),
                is_external: false,
            },
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::Number { value: "7".into() }),
                }],
            },
        };
        let main = ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Let {
                        name: "job".into(),
                        value: Some(ExecutableExpr::Call {
                            callee: Callee::Name("worker".into()),
                            receiver: None,
                            args: Vec::new(),
                            type_name: Some("Int".into()),
                        }),
                        is_async: true,
                    },
                    ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Await {
                            value: Box::new(ExecutableExpr::Ident {
                                name: "job".into(),
                                type_name: Some("Task<Int>".into()),
                            }),
                            type_name: Some("Int".into()),
                        }),
                    },
                ],
            },
        };
        let mir = lower_executable_ir_to_mir(&module_with_functions(vec![main, worker]))
            .expect("lower async binding");
        let main = mir
            .functions()
            .iter()
            .find(|function| {
                mir.function_debug(function.id())
                    .is_some_and(|debug| debug.name() == "main")
            })
            .expect("main function");
        assert!(matches!(
            main.blocks()[0].instructions(),
            [MirInstruction::Spawn { .. }, MirInstruction::Await { .. }]
        ));
        mir.verify().expect("verify structured task lifetime");
    }

    #[test]
    fn lowers_direct_calls_to_function_ids() {
        let executable = module_with_functions(vec![
            ExecutableFunction {
                name: "main".into(),
                is_async: false,
                signature: signature(),
                body: ExecutableBlock {
                    statements: vec![ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Call {
                            callee: Callee::Name("helper".into()),
                            receiver: None,
                            args: Vec::new(),
                            type_name: Some("Int".into()),
                        }),
                    }],
                },
            },
            ExecutableFunction {
                name: "helper".into(),
                is_async: false,
                signature: signature(),
                body: ExecutableBlock {
                    statements: vec![ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                    }],
                },
            },
        ]);

        let mir = lower_executable_ir_to_mir(&executable).unwrap();
        let instructions = mir.functions()[1].blocks()[0].instructions();
        assert!(matches!(
            instructions,
            [MirInstruction::Call {
                target: MirCallTarget::Function(target),
                ..
            }] if *target == FunctionId::new(0)
        ));
    }
}
