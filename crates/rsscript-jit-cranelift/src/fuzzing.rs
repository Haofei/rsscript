//! Coverage-guided probes for the detached `fuzz/` workspace.
//!
//! This module exists only behind the non-product `fuzzing` feature. It does not
//! expose raw executable pointers or permit execution; accepted scalar IR is
//! translated and finalized inside a short-lived, hard-bounded native module.

use crate::{JitError, JitFunction, JitLimits, NativeModule};

/// Validate and finalize a scalar JIT function under deterministic structural
/// limits and a small executable-memory arena.
///
/// The caller must generate IR without [`crate::JitInstr::HostCall`] or native
/// callees. Invalid or unsupported functions return a typed [`JitError`]. Machine
/// code is never invoked and is released when this function returns.
pub fn validate_and_codegen_scalar(
    function: &JitFunction,
    limits: JitLimits,
) -> Result<(), JitError> {
    let mut module = NativeModule::new_for_scalar_fuzzing(limits)?;
    module.compile(function).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JitInstr, JitValueType};

    #[test]
    fn scalar_probe_finalizes_without_a_host_helper_table() {
        let function = JitFunction {
            n_params: 1,
            n_regs: 2,
            reg_types: vec![JitValueType::Int; 2],
            zero_init_regs: Vec::new(),
            code: vec![
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 0 },
            ],
            instruction_origins: Vec::new(),
            source_instruction_count: 0,
            memo_scopes: Vec::new(),
            cold_blocks: Vec::new(),
            resume_live_regs: Vec::new(),
        };
        validate_and_codegen_scalar(&function, JitLimits::default()).unwrap();
    }
}
