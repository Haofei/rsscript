#![forbid(unsafe_code)]

//! Checked-HIR to provider-neutral MIR lowering.
//!
//! Direct checked-HIR to MIR lowering is the default closure. The historical
//! source-shaped `rsscript-exec-ir` projection remains available only through
//! the explicit `legacy-exec-ir` compatibility feature while parity work is
//! in progress.

mod mir;
#[cfg(feature = "legacy-exec-ir")]
mod projection;

#[cfg(feature = "legacy-exec-ir")]
pub use mir::lower_executable_ir_to_mir;
pub use mir::{MirLoweringError, lower_checked_hir_to_mir};
#[cfg(feature = "legacy-exec-ir")]
pub use rsscript_exec_ir::*;

#[cfg(feature = "legacy-exec-ir")]
pub fn lower_validated_hir(typed_hir: &rsscript_semantics::hir::Hir) -> ExecutableIr {
    let (program, external_imports) = projection::project_hir(typed_hir);
    ExecutableIr::new(program, external_imports)
}
