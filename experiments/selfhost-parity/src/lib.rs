#![forbid(unsafe_code)]

#[cfg(test)]
mod diagnostic {
    pub use rsscript_diagnostics::*;
}

#[cfg(test)]
mod interface_metadata;

#[cfg(test)]
mod interfaces {
    pub(crate) fn default_interfaces() -> impl Iterator<Item = (&'static str, &'static str)> {
        rsscript_interface_catalog::CORE_INTERFACES
            .iter()
            .chain(rsscript_interface_catalog::STANDARD_PACKAGE_INTERFACES.iter())
            .chain(TEST_INTERFACES.iter())
            .copied()
    }

    pub(crate) fn standard_package_interfaces() -> impl Iterator<Item = (&'static str, &'static str)>
    {
        rsscript_interface_catalog::STANDARD_PACKAGE_INTERFACES
            .iter()
            .chain(TEST_INTERFACES.iter())
            .copied()
    }

    const TEST_INTERFACES: &[(&str, &str)] = &[(
        "test/output.rssi",
        include_str!("../../../stdlib/output/output.rssi"),
    )];
}

#[cfg(test)]
mod lexer {
    pub(crate) use rsscript_syntax::lexer::*;
}

#[cfg(test)]
mod syntax {
    pub(crate) use rsscript_syntax::*;
}

#[cfg(test)]
mod vm_adapter {
    use rsscript_vm::{EvalError, RegVmExecutable};

    pub(crate) fn reg_vm_compile_sources(
        sources: &[(&str, &str)],
    ) -> Result<RegVmExecutable, EvalError> {
        let interfaces = crate::interfaces::standard_package_interfaces().collect::<Vec<_>>();
        let validated = rsscript_semantics::validate_sources_with_interfaces(sources, &interfaces)
            .map_err(EvalError::Diagnostics)?;
        let snapshot_digest = format!("sha256:{}", "0".repeat(64));
        let artifact =
            rsscript_compiler::compile_validated_to_bytecode(&validated, &snapshot_digest)
                .map_err(|error| EvalError::Runtime(error.to_string()))?;
        let bytes = artifact
            .to_bytes()
            .map_err(|error| EvalError::Runtime(error.to_string()))?;
        let verified = rsscript_bytecode::BytecodeVerifier::default()
            .verify(&bytes)
            .map_err(|error| EvalError::Runtime(error.to_string()))?;
        RegVmExecutable::from_verified_bytecode(verified)
    }
}

#[cfg(test)]
use rsscript_compiler::{Severity, analyze_source};
#[cfg(test)]
use rsscript_vm::RegVmExecutable;

#[cfg(test)]
mod selfhost_parity;
