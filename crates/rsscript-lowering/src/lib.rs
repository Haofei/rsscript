#![forbid(unsafe_code)]

//! Checked-HIR to provider-neutral executable-IR projection.
//!
//! The owned model lives in `rsscript-exec-ir`, which allows VM and optional
//! backends to consume it without pulling in syntax or semantic databases.

mod mir;
mod projection;

pub use mir::{MirLoweringError, lower_checked_hir_linear_to_mir, lower_executable_ir_to_mir};
pub use rsscript_exec_ir::*;

pub fn lower_validated_hir(typed_hir: &rsscript_semantics::hir::Hir) -> ExecutableIr {
    let (program, external_imports) = projection::project_hir(typed_hir);
    ExecutableIr::new(program, external_imports)
}
