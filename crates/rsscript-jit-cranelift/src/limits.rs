/// Deterministic structural limits applied before JIT analysis or Cranelift
/// code generation. These are work limits, not wall-clock deadlines: hostile or
/// accidentally huge IR is rejected before analysis can consume unbounded CPU
/// or memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitLimits {
    pub max_instructions: usize,
    pub max_registers: usize,
    pub max_parameters: usize,
    pub max_cfg_edges: usize,
    pub max_total_operands: usize,
    pub max_analysis_cells: usize,
    pub max_deopt_payload_words: usize,
    pub max_memo_scopes: usize,
    pub max_memo_slots: usize,
    pub max_native_callees: usize,
    pub max_group_members: usize,
    pub max_ir_work_units: u64,
}

impl Default for JitLimits {
    fn default() -> Self {
        Self {
            max_instructions: 1_000_000,
            max_registers: 65_536,
            max_parameters: 16_384,
            max_cfg_edges: 2_000_000,
            max_total_operands: 8_000_000,
            max_analysis_cells: 1_000_000,
            max_deopt_payload_words: 1_000_000,
            max_memo_scopes: 4_096,
            max_memo_slots: 65_536,
            max_native_callees: 4_096,
            max_group_members: 1_024,
            max_ir_work_units: 16_000_000,
        }
    }
}

impl JitLimits {
    pub(crate) fn checked_work(
        &self,
        instructions: usize,
        registers: usize,
        cfg_edges: usize,
        operands: usize,
        memo_scopes: usize,
    ) -> Option<u64> {
        let cells = instructions.checked_mul(registers)?;
        let memo_scan = memo_scopes.checked_mul(instructions.checked_add(cfg_edges)?)?;
        let total = cells
            .checked_add(cfg_edges)?
            .checked_add(operands)?
            .checked_add(memo_scan)?;
        u64::try_from(total).ok()
    }
}
