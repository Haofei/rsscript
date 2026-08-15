//! Test-only migration support for the typed MIR rollout.
//!
//! This deliberately small interpreter is an oracle for the currently
//! migrated pure subset. It is not a production VM and is only compiled with
//! the `conformance` feature. Migration tests use it to compare the legacy VM
//! path with the typed MIR path before a capability can become MIR-only.

use std::collections::BTreeSet;
use std::fmt;

use crate::{
    FunctionId, MirBinaryOp, MirCallArgument, MirCallTarget, MirInstruction, MirLiteral, MirModule,
    MirTerminator, ValueId,
};

/// Lifecycle of a language capability during the MIR migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStage {
    /// The feature remains only on the legacy executable-IR path.
    LegacyOnly,
    /// Both paths execute it and are required to agree in the conformance
    /// corpus.
    DualPath,
    /// The feature has a MIR execution path and may no longer add legacy-only
    /// behavior.
    MirOnly,
}

/// Declarative entry used by the migration corpus.
#[derive(Debug, Clone, Copy)]
pub struct MigrationCase {
    pub name: &'static str,
    pub capability: &'static str,
    pub stage: MigrationStage,
    pub source: &'static str,
}

/// The executable evidence required for a migrated capability.
///
/// The default requirement is deliberately the strongest one: the checked-HIR
/// MIR must agree with both the legacy VM and the small, pure MIR reference
/// interpreter, then execute through the verified-bytecode VM.  A capability
/// may opt out of the reference interpreter only when that interpreter does
/// not model a required runtime primitive yet.  It still has to prove
/// legacy/direct verified-bytecode parity, and the exception must state why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationEvidence {
    /// Compare legacy VM, pure MIR reference interpreter, and direct MIR
    /// verified-bytecode VM output.
    ReferenceInterpreterAndBytecode,
    /// Compare legacy VM and direct MIR verified-bytecode VM output. The
    /// rationale is a reviewed gap in the *test-only* reference interpreter,
    /// not permission to route through legacy lowering.
    VerifiedBytecode {
        reference_interpreter_gap: &'static str,
    },
}

/// A named exception to the default migration evidence requirement.
///
/// This remains a separate manifest because the overwhelmingly common case is
/// full reference-interpreter parity. The gate rejects unused, duplicate, or
/// unexplained exceptions so capabilities cannot silently disappear from the
/// stronger evidence path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationEvidenceOverride {
    pub case: &'static str,
    pub evidence: MigrationEvidence,
}

/// Structural failure in the declarative replacement corpus.
///
/// The old executable-IR bridge cannot be removed based on a test that merely
/// filters to the cases it happens to exercise. Before a capability is part of
/// the reviewed Core migration corpus, it must have one named source fixture
/// and remain on the dual path until the compatibility bridge is deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationGateError {
    EmptyCorpus,
    EmptyName,
    EmptyCapability {
        name: &'static str,
    },
    EmptySource {
        name: &'static str,
    },
    DuplicateName {
        name: &'static str,
    },
    DuplicateCapability {
        capability: &'static str,
    },
    NotDualPath {
        name: &'static str,
        stage: MigrationStage,
    },
    EmptyEvidenceCase,
    UnknownEvidenceCase {
        case: &'static str,
    },
    DuplicateEvidenceCase {
        case: &'static str,
    },
    RedundantEvidenceOverride {
        case: &'static str,
    },
    EmptyReferenceInterpreterGap {
        case: &'static str,
    },
}

impl fmt::Display for MigrationGateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCorpus => formatter.write_str("MIR replacement corpus is empty"),
            Self::EmptyName => formatter.write_str("MIR migration case has an empty name"),
            Self::EmptyCapability { name } => {
                write!(
                    formatter,
                    "MIR migration case `{name}` has an empty capability"
                )
            }
            Self::EmptySource { name } => {
                write!(
                    formatter,
                    "MIR migration case `{name}` has an empty source fixture"
                )
            }
            Self::DuplicateName { name } => {
                write!(formatter, "MIR migration corpus repeats case `{name}`")
            }
            Self::DuplicateCapability { capability } => {
                write!(
                    formatter,
                    "MIR migration corpus repeats capability `{capability}`"
                )
            }
            Self::NotDualPath { name, stage } => write!(
                formatter,
                "MIR migration case `{name}` is {stage:?}; replacement requires DualPath parity"
            ),
            Self::EmptyEvidenceCase => {
                formatter.write_str("MIR migration evidence override has an empty case name")
            }
            Self::UnknownEvidenceCase { case } => write!(
                formatter,
                "MIR migration evidence override refers to unknown case `{case}`"
            ),
            Self::DuplicateEvidenceCase { case } => write!(
                formatter,
                "MIR migration corpus repeats evidence override for `{case}`"
            ),
            Self::RedundantEvidenceOverride { case } => write!(
                formatter,
                "MIR migration evidence override for `{case}` repeats the default requirement"
            ),
            Self::EmptyReferenceInterpreterGap { case } => write!(
                formatter,
                "MIR migration evidence override for `{case}` does not explain its reference-interpreter gap"
            ),
        }
    }
}

impl std::error::Error for MigrationGateError {}

/// Validate the complete corpus used to decide whether the legacy lowering
/// may be removed.
///
/// This intentionally rejects both `LegacyOnly` and `MirOnly`: the gate is
/// about proving old/new observable parity *before* deletion. Once the old
/// bridge has been removed, this migration-only manifest and gate disappear
/// with it rather than silently treating a one-path fixture as parity proof.
pub fn require_dual_path_parity(cases: &[MigrationCase]) -> Result<(), MigrationGateError> {
    if cases.is_empty() {
        return Err(MigrationGateError::EmptyCorpus);
    }
    let mut names = BTreeSet::new();
    let mut capabilities = BTreeSet::new();
    for case in cases {
        if case.name.trim().is_empty() {
            return Err(MigrationGateError::EmptyName);
        }
        if case.capability.trim().is_empty() {
            return Err(MigrationGateError::EmptyCapability { name: case.name });
        }
        if case.source.trim().is_empty() {
            return Err(MigrationGateError::EmptySource { name: case.name });
        }
        if !names.insert(case.name) {
            return Err(MigrationGateError::DuplicateName { name: case.name });
        }
        if !capabilities.insert(case.capability) {
            return Err(MigrationGateError::DuplicateCapability {
                capability: case.capability,
            });
        }
        if case.stage != MigrationStage::DualPath {
            return Err(MigrationGateError::NotDualPath {
                name: case.name,
                stage: case.stage,
            });
        }
    }
    Ok(())
}

/// Validate reviewed exceptions to the default reference-interpreter evidence.
///
/// Call [`migration_evidence_for`] after this gate. A case without an override
/// always requires the full reference-interpreter plus verified-bytecode
/// comparison. This deliberately makes a weaker test path an explicit,
/// explained manifest change rather than an ad-hoc `if` in a test loop.
pub fn require_declared_migration_evidence(
    cases: &[MigrationCase],
    overrides: &[MigrationEvidenceOverride],
) -> Result<(), MigrationGateError> {
    require_dual_path_parity(cases)?;
    let names = cases.iter().map(|case| case.name).collect::<BTreeSet<_>>();
    let mut overridden = BTreeSet::new();
    for override_case in overrides {
        if override_case.case.trim().is_empty() {
            return Err(MigrationGateError::EmptyEvidenceCase);
        }
        if !names.contains(override_case.case) {
            return Err(MigrationGateError::UnknownEvidenceCase {
                case: override_case.case,
            });
        }
        if !overridden.insert(override_case.case) {
            return Err(MigrationGateError::DuplicateEvidenceCase {
                case: override_case.case,
            });
        }
        match override_case.evidence {
            MigrationEvidence::ReferenceInterpreterAndBytecode => {
                return Err(MigrationGateError::RedundantEvidenceOverride {
                    case: override_case.case,
                });
            }
            MigrationEvidence::VerifiedBytecode {
                reference_interpreter_gap,
            } if reference_interpreter_gap.trim().is_empty() => {
                return Err(MigrationGateError::EmptyReferenceInterpreterGap {
                    case: override_case.case,
                });
            }
            MigrationEvidence::VerifiedBytecode { .. } => {}
        }
    }
    Ok(())
}

/// Return the reviewed evidence requirement for a migration case.
///
/// The caller must first validate `overrides` with
/// [`require_declared_migration_evidence`].
pub fn migration_evidence_for(
    case: &MigrationCase,
    overrides: &[MigrationEvidenceOverride],
) -> MigrationEvidence {
    overrides
        .iter()
        .find(|override_case| override_case.case == case.name)
        .map(|override_case| override_case.evidence)
        .unwrap_or(MigrationEvidence::ReferenceInterpreterAndBytecode)
}

/// Scalar value model used only by the reference interpreter.
#[derive(Debug, Clone, PartialEq)]
pub enum MirValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    List(Vec<MirValue>),
    Map(Vec<(MirValue, MirValue)>),
    JsonObject(Vec<(String, MirValue)>),
    Record {
        name: String,
        fields: Vec<(String, MirValue)>,
    },
    Variant {
        name: String,
        fields: Vec<(String, MirValue)>,
    },
    ResultOk(Box<MirValue>),
    ResultErr(Box<MirValue>),
    OptionSome(Box<MirValue>),
    OptionNone,
}

impl MirValue {
    /// Render using the legacy VM's scalar result convention.
    pub fn render(&self) -> String {
        match self {
            Self::Unit => "Unit".into(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::Char(value) => value.to_string(),
            Self::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Map(entries) => {
                let mut rendered = entries
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.render(), value.render()))
                    .collect::<Vec<_>>();
                rendered.sort();
                format!("{{{}}}", rendered.join(", "))
            }
            Self::JsonObject(fields) => {
                let mut rendered = fields
                    .iter()
                    .map(|(name, value)| format!("\"{name}\":{}", value.render()))
                    .collect::<Vec<_>>();
                rendered.sort();
                format!("{{{}}}", rendered.join(","))
            }
            Self::Record { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({fields})")
            }
            Self::Variant { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({fields})")
            }
            Self::ResultOk(value) => format!("Ok({})", value.render()),
            Self::ResultErr(value) => format!("Err({})", value.render()),
            Self::OptionSome(value) => format!("Some({})", value.render()),
            Self::OptionNone => "None".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirExecutionError {
    MissingEntrypoint(String),
    InvalidArgumentCount {
        function: FunctionId,
        expected: usize,
        actual: usize,
    },
    UninitializedValue(ValueId),
    UninitializedPlace(usize),
    InvalidBranchCondition,
    InvalidOperation(&'static str),
    DivisionByZero,
    UnsupportedExternalCall,
    UnsupportedBuiltinCall,
    UnsupportedStructuredConcurrency,
    RecursionLimit,
    StepLimit,
}

impl fmt::Display for MirExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MIR conformance execution failed: {self:?}")
    }
}

impl std::error::Error for MirExecutionError {}

/// Execute a pure MIR module from its debug-name entry point.
pub fn execute_named(
    module: &MirModule,
    entry: &str,
    arguments: Vec<MirValue>,
) -> Result<MirValue, MirExecutionError> {
    let function = module
        .functions()
        .iter()
        .find(|function| {
            module
                .function_debug(function.id())
                .is_some_and(|debug| debug.name() == entry)
        })
        .map(|function| function.id())
        .ok_or_else(|| MirExecutionError::MissingEntrypoint(entry.into()))?;
    let mut interpreter = Interpreter {
        module,
        steps_remaining: 100_000,
        recursion_remaining: 128,
    };
    interpreter
        .call(function, arguments)
        .map(|outcome| outcome.value)
}

struct FrameOutcome {
    value: MirValue,
    places: Vec<Option<MirValue>>,
}

struct Interpreter<'a> {
    module: &'a MirModule,
    steps_remaining: u64,
    recursion_remaining: u32,
}

impl<'a> Interpreter<'a> {
    fn call(
        &mut self,
        function_id: FunctionId,
        arguments: Vec<MirValue>,
    ) -> Result<FrameOutcome, MirExecutionError> {
        if self.recursion_remaining == 0 {
            return Err(MirExecutionError::RecursionLimit);
        }
        self.recursion_remaining -= 1;
        let result = self.call_inner(function_id, arguments);
        self.recursion_remaining += 1;
        result
    }

    fn call_inner(
        &mut self,
        function_id: FunctionId,
        arguments: Vec<MirValue>,
    ) -> Result<FrameOutcome, MirExecutionError> {
        let function = self
            .module
            .function(function_id)
            .expect("verified MIR function target must exist");
        if arguments.len() != function.signature().parameter_types().len() {
            return Err(MirExecutionError::InvalidArgumentCount {
                function: function_id,
                expected: function.signature().parameter_types().len(),
                actual: arguments.len(),
            });
        }
        let mut places = vec![None; function.place_count() as usize];
        for (index, value) in arguments.into_iter().enumerate() {
            places[index] = Some(value);
        }
        let mut values = vec![None; function.value_count() as usize];
        let mut block = 0usize;
        loop {
            let current = &function.blocks()[block];
            for instruction in current.instructions() {
                self.step()?;
                match instruction {
                    MirInstruction::LoadLiteral { destination, value } => {
                        values[destination.index()] = Some(literal(value));
                    }
                    MirInstruction::MakeList { destination, items } => {
                        values[destination.index()] = Some(MirValue::List(
                            items
                                .iter()
                                .map(|item| value_at(&values, *item))
                                .collect::<Result<Vec<_>, _>>()?,
                        ));
                    }
                    MirInstruction::MakeMap {
                        destination,
                        entries,
                    } => {
                        values[destination.index()] = Some(MirValue::Map(
                            entries
                                .iter()
                                .map(|(key, value)| {
                                    Ok((value_at(&values, *key)?, value_at(&values, *value)?))
                                })
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        ));
                    }
                    MirInstruction::MakeObject {
                        destination,
                        fields,
                    } => {
                        values[destination.index()] = Some(MirValue::JsonObject(
                            fields
                                .iter()
                                .map(|(name, value)| Ok((name.clone(), value_at(&values, *value)?)))
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        ));
                    }
                    MirInstruction::MakeStruct {
                        destination,
                        ty,
                        fields,
                    } => {
                        let name = match self.module.ty(*ty) {
                            Some(rsscript_abi_model::WireType::Named { name, .. }) => name.clone(),
                            _ => return Err(MirExecutionError::InvalidOperation("record type")),
                        };
                        values[destination.index()] = Some(MirValue::Record {
                            name,
                            fields: fields
                                .iter()
                                .map(|(field, value)| {
                                    Ok((field.clone(), value_at(&values, *value)?))
                                })
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        });
                    }
                    MirInstruction::MakeVariant {
                        destination,
                        variant,
                        fields,
                        ..
                    } => {
                        values[destination.index()] = Some(MirValue::Variant {
                            name: variant.clone(),
                            fields: fields
                                .iter()
                                .map(|(field, value)| {
                                    Ok((field.clone(), value_at(&values, *value)?))
                                })
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        });
                    }
                    MirInstruction::MakeResult {
                        destination,
                        ok,
                        value,
                    } => {
                        let value = value_at(&values, *value)?;
                        values[destination.index()] = Some(if *ok {
                            MirValue::ResultOk(Box::new(value))
                        } else {
                            MirValue::ResultErr(Box::new(value))
                        });
                    }
                    MirInstruction::UnwrapResult {
                        destination,
                        source,
                        ok,
                    } => match value_at(&values, *source)? {
                        MirValue::ResultOk(value) if *ok => {
                            values[destination.index()] = Some(*value);
                        }
                        MirValue::ResultErr(value) if !*ok => {
                            values[destination.index()] = Some(*value);
                        }
                        _ => {
                            return Err(MirExecutionError::InvalidOperation(
                                "Result arm projection",
                            ));
                        }
                    },
                    MirInstruction::MakeOption { destination, value } => {
                        values[destination.index()] = Some(match value {
                            Some(value) => {
                                MirValue::OptionSome(Box::new(value_at(&values, *value)?))
                            }
                            None => MirValue::OptionNone,
                        });
                    }
                    MirInstruction::UnwrapOption {
                        destination,
                        source,
                    } => match value_at(&values, *source)? {
                        MirValue::OptionSome(value) => {
                            values[destination.index()] = Some(*value);
                        }
                        _ => {
                            return Err(MirExecutionError::InvalidOperation(
                                "Option arm projection",
                            ));
                        }
                    },
                    MirInstruction::TryResult {
                        destination,
                        source,
                        cleanup,
                    } => match value_at(&values, *source)? {
                        MirValue::ResultOk(value) => {
                            values[destination.index()] = Some(*value);
                        }
                        failure @ MirValue::ResultErr(_) => {
                            for place in cleanup {
                                let _ = places[place.index()]
                                    .take()
                                    .ok_or(MirExecutionError::UninitializedPlace(place.index()))?;
                            }
                            return Ok(FrameOutcome {
                                value: failure,
                                places,
                            });
                        }
                        _ => return Err(MirExecutionError::InvalidOperation("Result try source")),
                    },
                    MirInstruction::ListGet {
                        destination,
                        list,
                        index,
                    } => {
                        let index = match value_at(&values, *index)? {
                            MirValue::Int(value) if value >= 0 => value as usize,
                            _ => return Err(MirExecutionError::InvalidOperation("list index")),
                        };
                        let value = match value_at(&values, *list)? {
                            MirValue::List(items) => items.get(index).cloned().ok_or(
                                MirExecutionError::InvalidOperation("list index out of bounds"),
                            )?,
                            _ => return Err(MirExecutionError::InvalidOperation("list base")),
                        };
                        values[destination.index()] = Some(value);
                    }
                    MirInstruction::ListAppend {
                        destination,
                        list,
                        values: appended,
                    } => {
                        let appended = match value_at(&values, *appended)? {
                            MirValue::List(values) => values,
                            _ => return Err(MirExecutionError::InvalidOperation("list append")),
                        };
                        let items = match places[list.index()].as_mut() {
                            Some(MirValue::List(items)) => items,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("list base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(list.index()));
                            }
                        };
                        items.extend(appended);
                        values[destination.index()] = Some(MirValue::Unit);
                    }
                    MirInstruction::ListClear { destination, list } => {
                        let items = match places[list.index()].as_mut() {
                            Some(MirValue::List(items)) => items,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("list base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(list.index()));
                            }
                        };
                        items.clear();
                        values[destination.index()] = Some(MirValue::Unit);
                    }
                    MirInstruction::ListPop { destination, list } => {
                        let items = match places[list.index()].as_mut() {
                            Some(MirValue::List(items)) => items,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("list base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(list.index()));
                            }
                        };
                        values[destination.index()] = Some(
                            items
                                .pop()
                                .map(|value| MirValue::OptionSome(Box::new(value)))
                                .unwrap_or(MirValue::OptionNone),
                        );
                    }
                    MirInstruction::ListPush {
                        destination,
                        list,
                        value,
                    } => {
                        let value = value_at(&values, *value)?;
                        let items = match places[list.index()].as_mut() {
                            Some(MirValue::List(items)) => items,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("list base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(list.index()));
                            }
                        };
                        items.push(value);
                        values[destination.index()] = Some(MirValue::Unit);
                    }
                    MirInstruction::ListRemoveAt {
                        destination,
                        list,
                        index,
                    } => {
                        let index = match value_at(&values, *index)? {
                            MirValue::Int(index) => index,
                            _ => return Err(MirExecutionError::InvalidOperation("list index")),
                        };
                        let items = match places[list.index()].as_mut() {
                            Some(MirValue::List(items)) => items,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("list base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(list.index()));
                            }
                        };
                        let removed = (index >= 0)
                            .then_some(index as usize)
                            .filter(|index| *index < items.len())
                            .map(|index| items.remove(index));
                        values[destination.index()] = Some(
                            removed
                                .map(|value| MirValue::OptionSome(Box::new(value)))
                                .unwrap_or(MirValue::OptionNone),
                        );
                    }
                    MirInstruction::ListSet {
                        destination,
                        list,
                        index,
                        value,
                    } => {
                        let index = match value_at(&values, *index)? {
                            MirValue::Int(index) if index >= 0 => index as usize,
                            _ => return Err(MirExecutionError::InvalidOperation("list index")),
                        };
                        let value = value_at(&values, *value)?;
                        let items = match places[list.index()].as_mut() {
                            Some(MirValue::List(items)) => items,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("list base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(list.index()));
                            }
                        };
                        let Some(slot) = items.get_mut(index) else {
                            return Err(MirExecutionError::InvalidOperation(
                                "list index out of bounds",
                            ));
                        };
                        *slot = value;
                        values[destination.index()] = Some(MirValue::Unit);
                    }
                    MirInstruction::SetClear { .. }
                    | MirInstruction::SetInsert { .. }
                    | MirInstruction::SetRemove { .. } => {
                        return Err(MirExecutionError::UnsupportedBuiltinCall);
                    }
                    MirInstruction::MapGet {
                        destination,
                        map,
                        key,
                    } => {
                        let key = value_at(&values, *key)?;
                        let value = match value_at(&values, *map)? {
                            MirValue::Map(entries) => entries
                                .into_iter()
                                .find_map(|(entry_key, entry_value)| {
                                    (entry_key == key).then_some(entry_value)
                                })
                                .map(|value| MirValue::OptionSome(Box::new(value)))
                                .unwrap_or(MirValue::OptionNone),
                            _ => return Err(MirExecutionError::InvalidOperation("map base")),
                        };
                        values[destination.index()] = Some(value);
                    }
                    MirInstruction::MapClear { destination, map } => {
                        let entries = match places[map.index()].as_mut() {
                            Some(MirValue::Map(entries)) => entries,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("map base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(map.index()));
                            }
                        };
                        entries.clear();
                        values[destination.index()] = Some(MirValue::Unit);
                    }
                    MirInstruction::MapInsert {
                        destination,
                        map,
                        key,
                        value,
                    } => {
                        let key = value_at(&values, *key)?;
                        let value = value_at(&values, *value)?;
                        let entries = match places[map.index()].as_mut() {
                            Some(MirValue::Map(entries)) => entries,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("map base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(map.index()));
                            }
                        };
                        if let Some((_, existing)) =
                            entries.iter_mut().find(|(entry_key, _)| *entry_key == key)
                        {
                            *existing = value;
                        } else {
                            entries.push((key, value));
                        }
                        values[destination.index()] = Some(MirValue::Unit);
                    }
                    MirInstruction::MapInsertOld {
                        destination,
                        map,
                        key,
                        value,
                    } => {
                        let key = value_at(&values, *key)?;
                        let value = value_at(&values, *value)?;
                        let entries = match places[map.index()].as_mut() {
                            Some(MirValue::Map(entries)) => entries,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("map base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(map.index()));
                            }
                        };
                        let old = if let Some((_, existing)) =
                            entries.iter_mut().find(|(entry_key, _)| *entry_key == key)
                        {
                            Some(std::mem::replace(existing, value))
                        } else {
                            entries.push((key, value));
                            None
                        };
                        values[destination.index()] = Some(
                            old.map(|value| MirValue::OptionSome(Box::new(value)))
                                .unwrap_or(MirValue::OptionNone),
                        );
                    }
                    MirInstruction::MapRemove {
                        destination,
                        map,
                        key,
                    } => {
                        let key = value_at(&values, *key)?;
                        let entries = match places[map.index()].as_mut() {
                            Some(MirValue::Map(entries)) => entries,
                            Some(_) => {
                                return Err(MirExecutionError::InvalidOperation("map base"));
                            }
                            None => {
                                return Err(MirExecutionError::UninitializedPlace(map.index()));
                            }
                        };
                        let old = entries
                            .iter()
                            .position(|(entry_key, _)| *entry_key == key)
                            .map(|index| entries.remove(index).1);
                        values[destination.index()] = Some(
                            old.map(|value| MirValue::OptionSome(Box::new(value)))
                                .unwrap_or(MirValue::OptionNone),
                        );
                    }
                    MirInstruction::GetField {
                        destination,
                        base,
                        field,
                    } => {
                        let value = match value_at(&values, *base)? {
                            MirValue::JsonObject(fields)
                            | MirValue::Record { fields, .. }
                            | MirValue::Variant { fields, .. } => fields
                                .into_iter()
                                .find_map(|(name, value)| (name == *field).then_some(value))
                                .ok_or(MirExecutionError::InvalidOperation(
                                    "missing object field",
                                ))?,
                            _ => return Err(MirExecutionError::InvalidOperation("field base")),
                        };
                        values[destination.index()] = Some(value);
                    }
                    MirInstruction::ListLen { destination, list } => {
                        let length = match value_at(&values, *list)? {
                            MirValue::List(items) => items.len() as i64,
                            _ => return Err(MirExecutionError::InvalidOperation("list base")),
                        };
                        values[destination.index()] = Some(MirValue::Int(length));
                    }
                    MirInstruction::ReadPlace { destination, place } => {
                        values[destination.index()] = Some(
                            places[place.index()]
                                .clone()
                                .ok_or(MirExecutionError::UninitializedPlace(place.index()))?,
                        );
                    }
                    MirInstruction::BorrowRead { destination, place } => {
                        values[destination.index()] = Some(
                            places[place.index()]
                                .clone()
                                .ok_or(MirExecutionError::UninitializedPlace(place.index()))?,
                        );
                    }
                    MirInstruction::TakePlace { destination, place } => {
                        values[destination.index()] = Some(
                            places[place.index()]
                                .take()
                                .ok_or(MirExecutionError::UninitializedPlace(place.index()))?,
                        );
                    }
                    // The conformance interpreter has value semantics and no
                    // mutable-cell representation. `Manage` still remains an
                    // explicit operation in the tested MIR; after the source
                    // ownership transition it preserves the resulting graph.
                    MirInstruction::Manage {
                        destination,
                        source,
                    } => {
                        values[destination.index()] = Some(value_at(&values, *source)?);
                    }
                    MirInstruction::Retain { place } => {
                        let _ = place_value(&places, place.index())?;
                    }
                    MirInstruction::Drop { place } => {
                        let _ = places[place.index()]
                            .take()
                            .ok_or(MirExecutionError::UninitializedPlace(place.index()))?;
                    }
                    MirInstruction::AcquireResource { place, source, .. } => {
                        // This migration oracle has no host resource implementation,
                        // but it preserves the acquired source value in the live place.
                        places[place.index()] = Some(value_at(&values, *source)?);
                    }
                    MirInstruction::ReleaseResource { place } => {
                        let _ = places[place.index()]
                            .take()
                            .ok_or(MirExecutionError::UninitializedPlace(place.index()))?;
                    }
                    MirInstruction::Spawn { .. }
                    | MirInstruction::Await { .. }
                    | MirInstruction::Select { .. }
                    | MirInstruction::Cancel { .. }
                    | MirInstruction::Join { .. } => {
                        return Err(MirExecutionError::UnsupportedStructuredConcurrency);
                    }
                    MirInstruction::WritePlace { place, value } => {
                        places[place.index()] = Some(value_at(&values, *value)?);
                    }
                    MirInstruction::Binary {
                        destination,
                        op,
                        left,
                        right,
                    } => {
                        values[destination.index()] = Some(binary(
                            *op,
                            value_at(&values, *left)?,
                            value_at(&values, *right)?,
                        )?);
                    }
                    MirInstruction::Call {
                        destination,
                        target,
                        arguments,
                    } => {
                        let mut call_arguments = Vec::with_capacity(arguments.len());
                        let mut writebacks = Vec::new();
                        for (index, argument) in arguments.iter().enumerate() {
                            match argument {
                                MirCallArgument::Value(value) => {
                                    call_arguments.push(value_at(&values, *value)?);
                                }
                                MirCallArgument::BorrowRead(place) => {
                                    call_arguments.push(place_value(&places, place.index())?);
                                }
                                MirCallArgument::BorrowMut(place) => {
                                    call_arguments.push(place_value(&places, place.index())?);
                                    writebacks.push((place.index(), index));
                                }
                                MirCallArgument::Take(place) => {
                                    call_arguments.push(places[place.index()].take().ok_or(
                                        MirExecutionError::UninitializedPlace(place.index()),
                                    )?);
                                }
                            }
                        }
                        let outcome = match target {
                            MirCallTarget::Function(function) => {
                                self.call(*function, call_arguments)?
                            }
                            MirCallTarget::External(_) => {
                                return Err(MirExecutionError::UnsupportedExternalCall);
                            }
                            MirCallTarget::Builtin { .. } => {
                                return Err(MirExecutionError::UnsupportedBuiltinCall);
                            }
                        };
                        for (caller_place, callee_parameter) in writebacks {
                            places[caller_place] = outcome.places[callee_parameter].clone();
                        }
                        values[destination.index()] = Some(outcome.value);
                    }
                    MirInstruction::Discard { value } => {
                        let _ = value_at(&values, *value)?;
                    }
                }
            }
            self.step()?;
            match current.terminator() {
                MirTerminator::Return(value) => {
                    let value = value
                        .map(|value| value_at(&values, value))
                        .transpose()
                        .map(|value| value.unwrap_or(MirValue::Unit))?;
                    return Ok(FrameOutcome { value, places });
                }
                MirTerminator::Jump(target) => block = target.index(),
                MirTerminator::Branch {
                    condition,
                    then_target,
                    else_target,
                } => match value_at(&values, *condition)? {
                    MirValue::Bool(true) => block = then_target.index(),
                    MirValue::Bool(false) => block = else_target.index(),
                    _ => return Err(MirExecutionError::InvalidBranchCondition),
                },
                MirTerminator::MatchVariant {
                    value,
                    expected,
                    match_target,
                    else_target,
                } => match value_at(&values, *value)? {
                    MirValue::Variant { name, .. } if name == *expected => {
                        block = match_target.index();
                    }
                    _ => block = else_target.index(),
                },
                MirTerminator::MatchResult {
                    value,
                    ok_target,
                    err_target,
                } => match value_at(&values, *value)? {
                    MirValue::ResultOk(_) => block = ok_target.index(),
                    MirValue::ResultErr(_) => block = err_target.index(),
                    _ => return Err(MirExecutionError::InvalidOperation("Result match")),
                },
                MirTerminator::MatchOption {
                    value,
                    some_target,
                    none_target,
                } => match value_at(&values, *value)? {
                    MirValue::OptionSome(_) => block = some_target.index(),
                    MirValue::OptionNone => block = none_target.index(),
                    _ => return Err(MirExecutionError::InvalidOperation("Option match")),
                },
                MirTerminator::Unreachable => {
                    return Err(MirExecutionError::InvalidOperation("unreachable"));
                }
            }
        }
    }

    fn step(&mut self) -> Result<(), MirExecutionError> {
        self.steps_remaining = self
            .steps_remaining
            .checked_sub(1)
            .ok_or(MirExecutionError::StepLimit)?;
        Ok(())
    }
}

fn value_at(values: &[Option<MirValue>], id: ValueId) -> Result<MirValue, MirExecutionError> {
    values[id.index()]
        .clone()
        .ok_or(MirExecutionError::UninitializedValue(id))
}

fn place_value(places: &[Option<MirValue>], index: usize) -> Result<MirValue, MirExecutionError> {
    places[index]
        .clone()
        .ok_or(MirExecutionError::UninitializedPlace(index))
}

fn literal(value: &MirLiteral) -> MirValue {
    match value {
        MirLiteral::Unit => MirValue::Unit,
        MirLiteral::Int(value) => MirValue::Int(*value),
        MirLiteral::Float(value) => MirValue::Float(*value),
        MirLiteral::Bool(value) => MirValue::Bool(*value),
        MirLiteral::String(value) => MirValue::String(value.clone()),
        MirLiteral::Char(value) => MirValue::Char(*value),
    }
}

fn binary(op: MirBinaryOp, left: MirValue, right: MirValue) -> Result<MirValue, MirExecutionError> {
    use MirBinaryOp as Op;
    use MirValue as Value;
    match (op, left, right) {
        (Op::Add, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left + right)),
        (Op::Subtract, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left - right)),
        (Op::Multiply, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left * right)),
        (Op::Divide, Value::Int(_), Value::Int(0)) | (Op::Modulo, Value::Int(_), Value::Int(0)) => {
            Err(MirExecutionError::DivisionByZero)
        }
        (Op::Divide, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left / right)),
        (Op::Modulo, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left % right)),
        (Op::BitAnd, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left & right)),
        (Op::BitOr, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left | right)),
        (Op::BitXor, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left ^ right)),
        (Op::ShiftLeft, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left << right)),
        (Op::ShiftRight, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left >> right)),
        (Op::Add, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left + right)),
        (Op::Subtract, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left - right)),
        (Op::Multiply, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left * right)),
        (Op::Divide, Value::Float(_), Value::Float(0.0)) => Err(MirExecutionError::DivisionByZero),
        (Op::Divide, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left / right)),
        (Op::Equal, left, right) => Ok(Value::Bool(left == right)),
        (Op::NotEqual, left, right) => Ok(Value::Bool(left != right)),
        (Op::Less, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left < right)),
        (Op::LessEqual, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left <= right)),
        (Op::Greater, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left > right)),
        (Op::GreaterEqual, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left >= right)),
        (Op::Less, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left < right)),
        (Op::LessEqual, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left <= right)),
        (Op::Greater, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left > right)),
        (Op::GreaterEqual, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Bool(left >= right))
        }
        (Op::LogicalAnd, Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(left && right)),
        (Op::LogicalOr, Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(left || right)),
        _ => Err(MirExecutionError::InvalidOperation("binary operand types")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BasicBlock, BlockId, MirFunction, MirFunctionDebug, MirFunctionSignature, TypeId, WireType,
    };

    const CASE: MigrationCase = MigrationCase {
        name: "scalar",
        capability: "scalar arithmetic",
        stage: MigrationStage::DualPath,
        source: "fn main() -> Int { return 1 }",
    };

    #[test]
    fn replacement_gate_requires_a_complete_unique_dual_path_manifest() {
        assert_eq!(require_dual_path_parity(&[CASE]), Ok(()));
        assert!(matches!(
            require_dual_path_parity(&[]),
            Err(MigrationGateError::EmptyCorpus)
        ));
        assert!(matches!(
            require_dual_path_parity(&[CASE, CASE]),
            Err(MigrationGateError::DuplicateName { .. })
        ));
        assert!(matches!(
            require_dual_path_parity(&[MigrationCase {
                stage: MigrationStage::LegacyOnly,
                ..CASE
            }]),
            Err(MigrationGateError::NotDualPath {
                stage: MigrationStage::LegacyOnly,
                ..
            })
        ));
        assert!(matches!(
            require_dual_path_parity(&[MigrationCase {
                stage: MigrationStage::MirOnly,
                ..CASE
            }]),
            Err(MigrationGateError::NotDualPath {
                stage: MigrationStage::MirOnly,
                ..
            })
        ));
    }

    #[test]
    fn evidence_exceptions_are_explicit_and_explained() {
        const BYTECODE_ONLY: MigrationEvidenceOverride = MigrationEvidenceOverride {
            case: "scalar",
            evidence: MigrationEvidence::VerifiedBytecode {
                reference_interpreter_gap: "test-only interpreter has no typed intrinsic model",
            },
        };
        assert_eq!(
            require_declared_migration_evidence(&[CASE], &[BYTECODE_ONLY]),
            Ok(())
        );
        assert_eq!(
            migration_evidence_for(&CASE, &[BYTECODE_ONLY]),
            BYTECODE_ONLY.evidence
        );
        assert!(matches!(
            require_declared_migration_evidence(
                &[CASE],
                &[MigrationEvidenceOverride {
                    case: "missing",
                    evidence: BYTECODE_ONLY.evidence,
                }],
            ),
            Err(MigrationGateError::UnknownEvidenceCase { .. })
        ));
        assert!(matches!(
            require_declared_migration_evidence(
                &[CASE],
                &[MigrationEvidenceOverride {
                    case: CASE.name,
                    evidence: MigrationEvidence::VerifiedBytecode {
                        reference_interpreter_gap: "",
                    },
                }],
            ),
            Err(MigrationGateError::EmptyReferenceInterpreterGap { .. })
        ));
        assert!(matches!(
            require_declared_migration_evidence(
                &[CASE],
                &[MigrationEvidenceOverride {
                    case: CASE.name,
                    evidence: MigrationEvidence::ReferenceInterpreterAndBytecode,
                }],
            ),
            Err(MigrationGateError::RedundantEvidenceOverride { .. })
        ));
    }

    #[test]
    fn result_try_returns_the_failure_from_the_current_frame() {
        let types = vec![
            WireType::Unit,
            WireType::Int {
                bits: 64,
                signed: true,
            },
            WireType::String,
            WireType::Result {
                ok: Box::new(WireType::Int {
                    bits: 64,
                    signed: true,
                }),
                error: Box::new(WireType::String),
            },
        ];
        let result = TypeId::new(3);
        let fail = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(vec![], result, false),
            0,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::String("boom".into()),
                    },
                    MirInstruction::MakeResult {
                        destination: ValueId::new(1),
                        ok: false,
                        value: ValueId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        let main = MirFunction::new(
            FunctionId::new(1),
            MirFunctionSignature::new(vec![], result, false),
            0,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Call {
                        destination: ValueId::new(0),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![],
                    },
                    MirInstruction::TryResult {
                        destination: ValueId::new(1),
                        source: ValueId::new(0),
                        cleanup: vec![],
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        let module = MirModule::new(
            types,
            vec![fail, main],
            vec![
                MirFunctionDebug::new("fail", vec![]),
                MirFunctionDebug::new("main", vec![]),
            ],
            vec![],
        )
        .expect("Result MIR should verify");

        assert_eq!(
            execute_named(&module, "main", vec![])
                .expect("execute")
                .render(),
            "Err(boom)"
        );
    }
}
