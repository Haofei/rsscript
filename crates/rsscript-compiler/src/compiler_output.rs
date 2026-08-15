use std::error::Error;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::{Diagnostic, ValidatedProgram};
use rsscript_semantics::CompilationSession;

/// Owned output of the platform-neutral compiler boundary.
///
/// It contains no VM state and can be consumed by a bytecode emitter, AOT
/// integration, analyzer, or test harness without linking an execution engine.
#[derive(Debug, Clone)]
pub struct CompiledIr {
    #[cfg(feature = "legacy-exec-ir")]
    executable: rsscript_lowering::ExecutableIr,
    /// Immutable checked semantic input retained only through the migration so
    /// the preferred MIR path can avoid the source-shaped compatibility IR.
    checked_hir: rsscript_semantics::hir::Hir,
    source_hash: String,
    interface_catalog_digest: String,
}

/// Failure while lowering checked compiler output into the provider-neutral
/// reference-bytecode Artifact. The compiler owns this boundary; the VM only
/// receives the Artifact after a separate verifier pass.
#[cfg(feature = "bytecode")]
#[derive(Debug)]
pub enum BytecodeCompileError {
    Mir(rsscript_lowering::MirLoweringError),
    Emit(rsscript_codegen_vm::CodegenError),
}

#[cfg(feature = "bytecode")]
impl fmt::Display for BytecodeCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mir(error) => write!(formatter, "cannot lower checked HIR to MIR: {error}"),
            Self::Emit(error) => write!(formatter, "cannot emit MIR bytecode: {error}"),
        }
    }
}

#[cfg(feature = "bytecode")]
impl Error for BytecodeCompileError {}

impl CompiledIr {
    /// Lowers the checked executable representation into the frontend-free
    /// typed CFG MIR migration boundary.
    ///
    /// The migration subset covers resolved calls, structured control flow,
    /// lexical resource scopes, and internal task-group spawn/await. Other
    /// unsupported semantic forms still fail closed.
    pub fn mir(&self) -> Result<rsscript_mir::VerifiedMir, rsscript_lowering::MirLoweringError> {
        self.checked_hir_mir()
    }

    /// Preferred projection-free checked-HIR path. Unsupported capabilities
    /// return an explicit lowering error so the caller can make a deliberate
    /// compatibility decision rather than accidentally rebuilding syntax.
    pub fn checked_hir_mir(
        &self,
    ) -> Result<rsscript_mir::VerifiedMir, rsscript_lowering::MirLoweringError> {
        rsscript_lowering::lower_checked_hir_to_mir(&self.checked_hir)
    }

    /// Transitional source-shaped compatibility representation.
    ///
    /// New backends must call [`Self::mir`] and consume only checked-HIR MIR.
    /// This value remains available solely for the explicitly gated legacy VM
    /// fallback while unsupported MIR constructs are being migrated.
    #[cfg(feature = "legacy-exec-ir")]
    #[doc(hidden)]
    pub fn legacy_executable(&self) -> &rsscript_lowering::ExecutableIr {
        &self.executable
    }

    /// Consume the transitional source-shaped compatibility representation.
    #[cfg(feature = "legacy-exec-ir")]
    #[doc(hidden)]
    pub fn into_legacy_executable(self) -> rsscript_lowering::ExecutableIr {
        self.executable
    }

    #[cfg(feature = "legacy-exec-ir")]
    #[deprecated(note = "use `mir` for new backends; this is legacy executable-IR compatibility")]
    #[doc(hidden)]
    pub fn executable(&self) -> &rsscript_lowering::ExecutableIr {
        self.legacy_executable()
    }

    #[cfg(feature = "legacy-exec-ir")]
    #[deprecated(note = "use `mir` for new backends; this is legacy executable-IR compatibility")]
    #[doc(hidden)]
    pub fn into_executable(self) -> rsscript_lowering::ExecutableIr {
        self.into_legacy_executable()
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }

    pub fn interface_catalog_digest(&self) -> &str {
        &self.interface_catalog_digest
    }
}

pub fn compile_source_to_ir(file: &str, source: &str) -> Result<CompiledIr, Vec<Diagnostic>> {
    let mut session = CompilationSession::default();
    session
        .set_file(file, source)
        .expect("compiler source path must be a normalized unique logical path");
    let validated = session.workspace_validated()?;
    Ok(compile_validated_to_ir(&validated))
}

pub fn compile_validated_to_ir(validated: &ValidatedProgram) -> CompiledIr {
    let checked_hir = validated.database().hir().clone();
    CompiledIr {
        #[cfg(feature = "legacy-exec-ir")]
        executable: rsscript_lowering::lower_validated_hir(&checked_hir),
        checked_hir,
        source_hash: source_hash(validated),
        interface_catalog_digest: crate::interfaces::interface_catalog_digest(),
    }
}

/// Compile a validated program directly into a provider-neutral bytecode
/// Artifact. This is intentionally independent of the VM interpreter: callers
/// must still pass the resulting bytes through `BytecodeVerifier` before
/// execution.
#[cfg(feature = "bytecode")]
pub fn compile_validated_to_bytecode(
    validated: &ValidatedProgram,
    snapshot_digest: &str,
) -> Result<rsscript_bytecode::BytecodeArtifact, BytecodeCompileError> {
    let compiled = compile_validated_to_ir(validated);
    compile_ir_to_bytecode(&compiled, snapshot_digest)
}

/// Emit bytecode from already-owned compiler output without constructing a VM
/// executable. Kept public for project/package adapters that have already
/// captured an immutable snapshot.
#[cfg(feature = "bytecode")]
pub fn compile_ir_to_bytecode(
    compiled: &CompiledIr,
    snapshot_digest: &str,
) -> Result<rsscript_bytecode::BytecodeArtifact, BytecodeCompileError> {
    let mir = compiled
        .checked_hir_mir()
        .map_err(BytecodeCompileError::Mir)?;
    let mut artifact = rsscript_codegen_vm::emit_artifact(
        &mir,
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(BytecodeCompileError::Emit)?;
    artifact
        .bind_snapshot_digest(snapshot_digest)
        .map_err(|error| {
            BytecodeCompileError::Emit(rsscript_codegen_vm::CodegenError::Bytecode(
                error.to_string(),
            ))
        })?;
    Ok(artifact)
}

#[cfg(feature = "package")]
pub fn compile_package_input_to_ir(
    input: &crate::package::PackageLoweringInput,
) -> Result<CompiledIr, Vec<Diagnostic>> {
    // Package compatibility remains a separate feature, but its frontend work
    // must still enter through the semantic-owned session boundary. The
    // session supplies the Core interface catalog; package interfaces are the
    // only explicit additions, matching the previous `without_core` call
    // without duplicating Core declarations in the captured input.
    let mut session = CompilationSession::default();
    for (path, contents) in &input.interfaces {
        session
            .set_interface(path, contents)
            .expect("package lowering input contains normalized unique interface paths");
    }
    for (path, contents) in &input.sources {
        session
            .set_file(path, contents)
            .expect("package lowering input contains normalized unique source paths");
    }
    let validated = session.workspace_validated()?;
    Ok(compile_validated_to_ir(&validated))
}

fn source_hash(validated: &ValidatedProgram) -> String {
    let mut input = Vec::new();
    for snapshot in [
        validated.database().sources(),
        validated.database().interfaces(),
    ] {
        for file in snapshot.files() {
            input.extend_from_slice(&(file.path().len() as u64).to_be_bytes());
            input.extend_from_slice(file.path().as_bytes());
            input.extend_from_slice(&(file.text().len() as u64).to_be_bytes());
            input.extend_from_slice(file.text().as_bytes());
        }
    }
    format!("sha256:{:x}", Sha256::digest(input))
}

#[cfg(test)]
mod tests {
    use super::compile_source_to_ir;
    use rsscript_mir::{MirBinaryOp, MirInstruction};

    #[test]
    fn pure_control_flow_compiles_to_verified_mir() {
        let compiled = compile_source_to_ir(
            "mir-test.rss",
            r#"
fn main() -> Int {
    let value = 1
    if value < 2 {
        return value + 1
    } else {
        return 0
    }
}
"#,
        )
        .expect("source should compile");

        let mir = compiled.mir().expect("pure control flow lowers to MIR");
        assert_eq!(mir.functions().len(), 1);
        assert!(mir.functions()[0].blocks().len() >= 4);
        mir.verify().expect("MIR remains structurally valid");
    }

    #[test]
    fn linear_scalar_program_uses_the_direct_checked_hir_mir_path() {
        let compiled = compile_source_to_ir(
            "direct-hir-mir.rss",
            r#"
fn main() -> Int {
    let left = 40
    let right = 2
    return left + right
}
"#,
        )
        .expect("source should compile");
        let mir = compiled
            .checked_hir_mir()
            .expect("linear scalar HIR lowers without executable IR");
        assert_eq!(mir.functions().len(), 1);
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                rsscript_mir::MirInstruction::LoadLiteral { .. },
                rsscript_mir::MirInstruction::WritePlace { .. },
                rsscript_mir::MirInstruction::LoadLiteral { .. },
                rsscript_mir::MirInstruction::WritePlace { .. },
                rsscript_mir::MirInstruction::ReadPlace { .. },
                rsscript_mir::MirInstruction::ReadPlace { .. },
                rsscript_mir::MirInstruction::Binary { .. },
            ]
        ));
        let debug = mir
            .function_debug(mir.functions()[0].id())
            .expect("direct lowering records function debug metadata");
        let source = debug
            .source()
            .expect("checked-HIR lowering records a source location");
        assert_eq!(source.file(), "direct-hir-mir.rss");
        assert!(source.length() > 0);
        mir.verify().expect("direct HIR MIR verifies");
    }

    #[test]
    fn logical_short_circuit_uses_cfg_not_a_logical_binary_opcode() {
        let compiled = compile_source_to_ir(
            "logical-mir.rss",
            r#"
fn main() -> Bool {
    let left = false && true
    let right = true || false
    return left || right
}
"#,
        )
        .expect("source should compile");

        let mir = compiled
            .checked_hir_mir()
            .expect("logical source should lower directly to MIR");
        assert!(
            mir.functions()
                .iter()
                .flat_map(|function| function.blocks())
                .flat_map(|block| block.instructions())
                .all(|instruction| !matches!(
                    instruction,
                    MirInstruction::Binary {
                        op: MirBinaryOp::LogicalAnd | MirBinaryOp::LogicalOr,
                        ..
                    }
                )),
            "short-circuit semantics must be represented by explicit CFG branches"
        );
        assert!(
            mir.functions()[0].blocks().len() >= 10,
            "each logical expression contributes right-hand, short-circuit, and join blocks"
        );
    }

    #[test]
    fn direct_checked_hir_path_lowers_branches_to_cfg() {
        let compiled = compile_source_to_ir(
            "direct-hir-branch.rss",
            r#"
fn main() -> Int {
    let value = 41
    if value < 42 {
        return value + 1
    } else {
        return 0
    }
}
"#,
        )
        .expect("source should compile");
        let mir = compiled
            .checked_hir_mir()
            .expect("checked HIR branch lowers without executable IR");
        assert!(mir.functions()[0].blocks().iter().any(|block| {
            matches!(
                block.terminator(),
                rsscript_mir::MirTerminator::Branch { .. }
            )
        }));
        assert!(mir.functions()[0].blocks().len() >= 4);
        mir.verify().expect("direct HIR branch MIR verifies");
    }

    #[test]
    fn direct_checked_hir_path_lowers_loops_to_cfg() {
        let compiled = compile_source_to_ir(
            "direct-hir-loop.rss",
            r#"
fn main() -> Int {
    let mut value = 0
    while value < 5 {
        value = value + 1
    }
    return value
}
"#,
        )
        .expect("source should compile");
        let mir = compiled
            .checked_hir_mir()
            .expect("checked HIR loop lowers without executable IR");
        assert!(mir.functions()[0].blocks().iter().any(|block| {
            matches!(
                block.terminator(),
                rsscript_mir::MirTerminator::Branch { .. }
            )
        }));
        assert!(mir.functions()[0].blocks().len() >= 4);
        mir.verify().expect("direct HIR loop MIR verifies");
    }

    #[test]
    fn direct_checked_hir_path_lowers_loop_break_and_continue() {
        let compiled = compile_source_to_ir(
            "direct-hir-loop-control.rss",
            r#"
fn main() -> Int {
    let mut value = 0
    while value < 10 {
        value = value + 1
        if value == 3 {
            continue
        }
        if value == 5 {
            break
        }
    }
    return value
}
"#,
        )
        .expect("source should compile");
        let mir = compiled
            .checked_hir_mir()
            .expect("checked HIR loop control lowers without executable IR");
        assert!(mir.functions()[0].blocks().len() >= 7);
        mir.verify().expect("direct HIR loop control MIR verifies");
    }

    #[test]
    fn direct_checked_hir_path_resolves_internal_call_targets_to_function_ids() {
        let compiled = compile_source_to_ir(
            "direct-hir-call.rss",
            r#"
fn helper() -> Int {
    return 42
}

fn main() -> Int {
    return helper()
}
"#,
        )
        .expect("source should compile");
        let mir = compiled
            .checked_hir_mir()
            .expect("internal call lowers from checked HIR");
        assert!(mir.functions().iter().any(|function| {
            function.blocks()[0]
                .instructions()
                .iter()
                .any(|instruction| {
                    matches!(
                        instruction,
                        rsscript_mir::MirInstruction::Call {
                            target: rsscript_mir::MirCallTarget::Function(_),
                            ..
                        }
                    )
                })
        }));
        mir.verify().expect("direct HIR call MIR verifies");
    }

    #[test]
    fn direct_checked_hir_path_preserves_mut_and_take_call_arguments() {
        let compiled = compile_source_to_ir(
            "direct-hir-effects.rss",
            r#"
fn increment_in_place(value: mut Int) -> Int {
    value = value + 1
    return value
}

fn consume(value: take Int) -> Int {
    return value
}

fn main() -> Int {
    let mut value = 40
    increment_in_place(value: mut value)
    local taken = 41
    return consume(value: take taken)
}
"#,
        )
        .expect("source should compile");
        let mir = compiled
            .checked_hir_mir()
            .expect("checked HIR effects lower without executable IR");
        assert!(mir.functions().iter().any(|function| {
            function.blocks().iter().flat_map(|block| block.instructions()).any(|instruction| {
                matches!(
                    instruction,
                    rsscript_mir::MirInstruction::Call {
                        arguments,
                        ..
                    } if arguments.iter().any(|argument| matches!(
                        argument,
                        rsscript_mir::MirCallArgument::BorrowMut(_) | rsscript_mir::MirCallArgument::Take(_)
                    ))
                )
            })
        }));
        mir.verify().expect("direct HIR effect MIR verifies");
    }
}
