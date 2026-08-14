//! Compiler-to-VM adapter.
//!
//! This is the only compiler module allowed to depend on both validated
//! frontend state and `rsscript-vm`. The VM consumes only owned executable IR.

use std::path::Path;

use rsscript_compiler::{
    CompiledIr, PackageLoweringInput, ValidatedProgram, compile_package_input_to_ir,
    compile_source_to_ir, compile_validated_to_ir, prepare_package_for_execution,
};

use rsscript_bytecode::BytecodeArtifact;
#[cfg(feature = "native-jit")]
#[allow(unused_imports)]
pub use rsscript_vm::with_native_cost_model_disabled;
use rsscript_vm::{EvalError, EvalOutput, ExternalFunction};
#[allow(unused_imports)]
pub use rsscript_vm::{RegVmExecutable, VmLimits};

pub fn reg_vm_compile_package(package_dir: &Path) -> Result<RegVmExecutable, EvalError> {
    let prepared = prepare_package_for_execution(package_dir).map_err(EvalError::Runtime)?;
    let input = prepared.into_lowering_input().map_err(EvalError::Runtime)?;
    reg_vm_compile_package_input(&input)
}

pub fn reg_vm_compile_package_input(
    input: &PackageLoweringInput,
) -> Result<RegVmExecutable, EvalError> {
    let compiled = compile_package_input_to_ir(input).map_err(EvalError::Diagnostics)?;
    emit_ir(&compiled)
}

pub fn reg_vm_compile_source(file: &str, source: &str) -> Result<RegVmExecutable, EvalError> {
    let compiled = compile_source_to_ir(file, source).map_err(EvalError::Diagnostics)?;
    emit_ir(&compiled)
}

pub fn reg_vm_compile_validated(
    validated: &ValidatedProgram,
) -> Result<RegVmExecutable, EvalError> {
    let compiled = compile_validated_to_ir(validated);
    emit_ir(&compiled)
}

/// Emit typed MIR through the verified bytecode path. This adapter is used by
/// the MIR migration gate and does not pass executable IR into the VM.
#[doc(hidden)]
pub fn reg_vm_compile_mir(
    mir: &rsscript_mir::MirModule,
    source_hash: &str,
    interface_catalog_digest: &str,
) -> Result<RegVmExecutable, EvalError> {
    emit_mir(mir, source_hash, interface_catalog_digest)
        .map_err(|error| EvalError::Runtime(error.to_string()))
}

fn emit_ir(compiled: &CompiledIr) -> Result<RegVmExecutable, EvalError> {
    match compiled.checked_hir_mir() {
        Ok(mir) => match emit_mir(
            &mir,
            compiled.source_hash(),
            compiled.interface_catalog_digest(),
        ) {
            Ok(executable) => Ok(executable),
            Err(rsscript_codegen_vm::CodegenError::Unsupported(_)) => {
                emit_legacy_executable_ir(compiled)
            }
            Err(error) => Err(EvalError::Runtime(error.to_string())),
        },
        Err(rsscript_lowering::MirLoweringError::Unsupported { .. }) => {
            emit_legacy_executable_ir(compiled)
        }
        Err(error) => Err(EvalError::Runtime(error.to_string())),
    }
}

/// Explicit migration-only bridge for checked-HIR constructs that do not yet
/// have a CFG MIR representation. A direct-HIR failure other than `Unsupported`
/// is never hidden by this compatibility encoder.
#[cfg(feature = "legacy-exec-ir")]
fn emit_legacy_executable_ir(compiled: &CompiledIr) -> Result<RegVmExecutable, EvalError> {
    rsscript_vm::compile_executable_ir(
        compiled.legacy_executable(),
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )
}

#[cfg(not(feature = "legacy-exec-ir"))]
fn emit_legacy_executable_ir(_: &CompiledIr) -> Result<RegVmExecutable, EvalError> {
    Err(EvalError::Runtime(
        "the checked program requires a legacy executable-IR operation; migrate it to MIR or enable the explicit `legacy-exec-ir` compatibility feature"
            .to_string(),
    ))
}

/// Build a provider-neutral Artifact from compiler output without first
/// constructing a VM executable.
///
/// The reviewed SDK uses this boundary for every MIR capability supported by
/// `rsscript-codegen-vm`: compiler -> MIR -> bytecode Artifact. The legacy
/// executable-IR fallback remains deliberately confined to this compatibility
/// adapter while the migration corpus still has unsupported MIR constructs.
pub(crate) fn emit_compiled_artifact(
    compiled: &CompiledIr,
    snapshot_digest: &str,
) -> Result<BytecodeArtifact, EvalError> {
    match compiled.checked_hir_mir() {
        Ok(mir) => match emit_mir_artifact(
            &mir,
            compiled.source_hash(),
            compiled.interface_catalog_digest(),
            snapshot_digest,
        ) {
            Ok(artifact) => Ok(artifact),
            Err(rsscript_codegen_vm::CodegenError::Unsupported(_)) => {
                emit_legacy_compiled_artifact(compiled, snapshot_digest)
            }
            Err(error) => Err(EvalError::Runtime(error.to_string())),
        },
        Err(rsscript_lowering::MirLoweringError::Unsupported { .. }) => {
            emit_legacy_compiled_artifact(compiled, snapshot_digest)
        }
        Err(error) => Err(EvalError::Runtime(error.to_string())),
    }
}

/// Explicit migration-only Artifact path for unsupported checked-HIR forms.
/// Its use is confined to the SDK compatibility adapter and remains behind
/// the VM's `legacy-exec-ir` feature.
#[cfg(feature = "legacy-exec-ir")]
fn emit_legacy_compiled_artifact(
    compiled: &CompiledIr,
    snapshot_digest: &str,
) -> Result<BytecodeArtifact, EvalError> {
    let mut executable = rsscript_vm::compile_executable_ir(
        compiled.legacy_executable(),
        compiled.source_hash(),
        compiled.interface_catalog_digest(),
    )?;
    executable.bind_snapshot_digest(snapshot_digest)?;
    let bytes = executable.to_bytecode()?;
    BytecodeArtifact::from_bytes(&bytes).map_err(|error| EvalError::Runtime(error.to_string()))
}

#[cfg(not(feature = "legacy-exec-ir"))]
fn emit_legacy_compiled_artifact(_: &CompiledIr, _: &str) -> Result<BytecodeArtifact, EvalError> {
    Err(EvalError::Runtime(
        "the checked program requires a legacy executable-IR operation; migrate it to MIR or enable the explicit `legacy-exec-ir` compatibility feature"
            .to_string(),
    ))
}

fn emit_mir(
    mir: &rsscript_mir::MirModule,
    source_hash: &str,
    interface_catalog_digest: &str,
) -> Result<RegVmExecutable, rsscript_codegen_vm::CodegenError> {
    let artifact = rsscript_codegen_vm::emit_artifact(
        mir,
        source_hash,
        interface_catalog_digest,
        env!("CARGO_PKG_VERSION"),
    )?;
    let bytes = artifact
        .to_bytes()
        .map_err(|error| rsscript_codegen_vm::CodegenError::Bytecode(error.to_string()))?;
    let verified = rsscript_bytecode::BytecodeVerifier::default()
        .verify(&bytes)
        .map_err(|error| rsscript_codegen_vm::CodegenError::Bytecode(error.to_string()))?;
    RegVmExecutable::from_verified_bytecode(verified)
        .map_err(|error| rsscript_codegen_vm::CodegenError::Bytecode(format!("{error:?}")))
}

fn emit_mir_artifact(
    mir: &rsscript_mir::MirModule,
    source_hash: &str,
    interface_catalog_digest: &str,
    snapshot_digest: &str,
) -> Result<BytecodeArtifact, rsscript_codegen_vm::CodegenError> {
    let mut artifact = rsscript_codegen_vm::emit_artifact(
        mir,
        source_hash,
        interface_catalog_digest,
        env!("CARGO_PKG_VERSION"),
    )?;
    artifact
        .bind_snapshot_digest(snapshot_digest)
        .map_err(|error| rsscript_codegen_vm::CodegenError::Bytecode(error.to_string()))?;
    Ok(artifact)
}

pub fn reg_vm_eval_source_main_with_args(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args(args)
}

pub fn reg_vm_eval_source_main(file: &str, source: &str) -> Result<EvalOutput, EvalError> {
    reg_vm_eval_source_main_with_args(file, source, std::iter::empty::<String>())
}

pub fn reg_vm_eval_source_main_with_limits(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    limits: VmLimits,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_limits(args, limits)
}

pub fn reg_vm_eval_source_main_with_args_and_external_bindings_and_limits(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    limits: VmLimits,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_and_external_bindings_and_limits(
        args,
        external_bindings,
        limits,
    )
}

pub fn reg_vm_eval_source_main_jit(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_jit(args)
}

pub fn reg_vm_eval_source_main_with_args_and_external_bindings(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?
        .eval_main_with_args_and_external_bindings(args, external_bindings)
}

pub fn reg_vm_eval_source_main_with_args_streaming_stdout(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_and_external_bindings_streaming_stdout(
        args,
        std::iter::empty::<(String, ExternalFunction)>(),
    )
}

pub fn reg_vm_eval_package_main_with_args_and_external_bindings_streaming_stdout(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
    external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_package(package_dir)?
        .eval_main_with_args_and_external_bindings_streaming_stdout(args, external_bindings)
}

pub fn reg_vm_eval_package_main_with_args(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_eval_package_main_with_args_and_external_bindings(
        package_dir,
        args,
        std::iter::empty::<(String, ExternalFunction)>(),
    )
}

pub fn reg_vm_eval_package_main_with_args_and_external_bindings(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
    external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_package(package_dir)?
        .eval_main_with_args_and_external_bindings(args, external_bindings)
}

pub fn reg_vm_eval_package_main_with_args_and_external_bindings_and_limits(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
    external_bindings: impl IntoIterator<Item = (impl Into<String>, ExternalFunction)>,
    limits: VmLimits,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_package(package_dir)?.eval_main_with_args_and_external_bindings_and_limits(
        args,
        external_bindings,
        limits,
    )
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_force_deopt(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_force_deopt(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_force_safepoint(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    safepoint: u32,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_force_safepoint(args, safepoint)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_force_all_safepoints(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_force_all_safepoints(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_precise(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_precise(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_osr(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_osr(args)
}

#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_osr_report(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<(EvalOutput, rsscript_vm::NativeStats, Vec<String>), EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_osr_report(args)
}
