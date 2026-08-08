//! Compiler-to-VM adapter.
//!
//! This is the only compiler module allowed to depend on both validated
//! frontend state and `rsscript-vm`. The VM consumes only owned executable IR.

use std::path::Path;

use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::analyzer::validate_sources_with_interfaces;
use crate::analyzer::{validate_source, validate_sources_with_interfaces_without_core};
use crate::interfaces::builtin_interfaces;
use crate::package::{PackageLoweringInput, prepare_package_for_execution};
use crate::semantic::ValidatedProgram;

#[cfg(feature = "native-jit")]
pub use rsscript_vm::with_native_cost_model_disabled;
use rsscript_vm::{EvalError, EvalOutput, ExternalFunction};
pub use rsscript_vm::{JitPlan, RegVmExecutable, VmLimits};

pub fn reg_vm_compile_package(package_dir: &Path) -> Result<RegVmExecutable, EvalError> {
    let prepared = prepare_package_for_execution(package_dir).map_err(EvalError::Runtime)?;
    let input = prepared.into_lowering_input().map_err(EvalError::Runtime)?;
    reg_vm_compile_package_input(&input)
}

pub fn reg_vm_compile_package_input(
    input: &PackageLoweringInput,
) -> Result<RegVmExecutable, EvalError> {
    let mut interface_refs = builtin_interfaces()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect::<Vec<_>>();
    interface_refs.extend(input.interfaces.iter().cloned());
    let source_refs = input
        .sources
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let interface_refs_borrowed = interface_refs
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let validated =
        validate_sources_with_interfaces_without_core(&source_refs, &interface_refs_borrowed)
            .map_err(EvalError::Diagnostics)?;
    reg_vm_compile_validated(&validated)
}

pub fn reg_vm_compile_source(file: &str, source: &str) -> Result<RegVmExecutable, EvalError> {
    let validated = validate_source(file, source).map_err(EvalError::Diagnostics)?;
    reg_vm_compile_validated(&validated)
}

pub fn reg_vm_compile_validated(
    validated: &ValidatedProgram,
) -> Result<RegVmExecutable, EvalError> {
    let executable = rsscript_lowering::lower_validated_hir(validated.database().hir());
    rsscript_vm::compile_executable_ir(
        &executable,
        &source_hash(validated),
        &crate::interfaces::interface_catalog_digest(),
    )
}

#[cfg(test)]
pub(crate) fn reg_vm_compile_sources(
    sources: &[(&str, &str)],
) -> Result<RegVmExecutable, EvalError> {
    let interfaces = crate::interfaces::standard_package_interfaces().collect::<Vec<_>>();
    let validated =
        validate_sources_with_interfaces(sources, &interfaces).map_err(EvalError::Diagnostics)?;
    reg_vm_compile_validated(&validated)
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
