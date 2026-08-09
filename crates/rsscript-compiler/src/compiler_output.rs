use sha2::{Digest, Sha256};

use crate::{Diagnostic, ValidatedProgram, validate_source};

/// Owned output of the platform-neutral compiler boundary.
///
/// It contains no VM state and can be consumed by a bytecode emitter, AOT
/// integration, analyzer, or test harness without linking an execution engine.
#[derive(Debug, Clone)]
pub struct CompiledIr {
    executable: rsscript_lowering::ExecutableIr,
    source_hash: String,
    interface_catalog_digest: String,
}

impl CompiledIr {
    /// Lowers the checked executable representation into the frontend-free
    /// typed CFG MIR migration boundary.
    ///
    /// The initial bridge intentionally supports only pure control flow. It
    /// rejects resource, async, and call operations until those operations
    /// have explicit MIR representations.
    pub fn mir(&self) -> Result<rsscript_mir::MirModule, rsscript_lowering::MirLoweringError> {
        rsscript_lowering::lower_executable_ir_to_mir(&self.executable)
    }

    pub fn executable(&self) -> &rsscript_lowering::ExecutableIr {
        &self.executable
    }

    pub fn into_executable(self) -> rsscript_lowering::ExecutableIr {
        self.executable
    }

    pub fn source_hash(&self) -> &str {
        &self.source_hash
    }

    pub fn interface_catalog_digest(&self) -> &str {
        &self.interface_catalog_digest
    }
}

pub fn compile_source_to_ir(file: &str, source: &str) -> Result<CompiledIr, Vec<Diagnostic>> {
    let validated = validate_source(file, source)?;
    Ok(compile_validated_to_ir(&validated))
}

pub fn compile_validated_to_ir(validated: &ValidatedProgram) -> CompiledIr {
    CompiledIr {
        executable: rsscript_lowering::lower_validated_hir(validated.database().hir()),
        source_hash: source_hash(validated),
        interface_catalog_digest: crate::interfaces::interface_catalog_digest(),
    }
}

#[cfg(feature = "execution")]
pub fn compile_package_input_to_ir(
    input: &crate::PackageLoweringInput,
) -> Result<CompiledIr, Vec<Diagnostic>> {
    let mut interfaces = crate::interfaces::builtin_interfaces()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect::<Vec<_>>();
    interfaces.extend(input.interfaces.iter().cloned());
    let sources = input
        .sources
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let interface_refs = interfaces
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let validated =
        crate::validate_sources_with_interfaces_without_core(&sources, &interface_refs)?;
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
}
