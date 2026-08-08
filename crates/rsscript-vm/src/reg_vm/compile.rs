//! Compiler-to-VM adapter.
//!
//! This is the only production module under `reg_vm` allowed to consume
//! frontend validation types. The VM model, verifier, linker, and executor
//! consume only owned executable IR or verified bytecode.

use std::path::Path;
use std::rc::Rc;

use sha2::{Digest, Sha256};

use super::{EvalError, RegUnit, RegVmExecutable, bytecode};
#[cfg(test)]
use crate::analyzer::validate_sources_with_interfaces;
use crate::analyzer::{validate_source, validate_sources_with_interfaces_without_core};
use crate::interfaces::builtin_interfaces;
use crate::package::{PackageLoweringInput, prepare_package_for_execution};
use crate::semantic::ValidatedProgram;

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
    let executable =
        rsscript_lowering::ExecutableIr::from_validated_hir(validated.database().hir());
    let lowered = RegUnit::lower(&executable)?;
    let verified = bytecode::encode_and_verify(
        &lowered,
        &source_hash(validated),
        &crate::interfaces::interface_catalog_digest(),
        &executable,
    )?;
    let (artifact, unit) = verified.into_parts();
    Ok(RegVmExecutable {
        unit: Rc::new(unit),
        artifact,
    })
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
