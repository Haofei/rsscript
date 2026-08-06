use super::*;

mod bodies;
mod effects;
mod signatures;
mod structured_types;

use bodies::*;
use effects::*;
pub(in crate::hir) use structured_types::*;

pub fn assign_target_reads(target: &HirExpr) -> Vec<&HirExpr> {
    structured_types::assign_target_reads_impl(target)
}
