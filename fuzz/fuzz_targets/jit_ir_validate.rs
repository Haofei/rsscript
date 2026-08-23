#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript_jit_cranelift::{
    JitCompare, JitFunction, JitInstr, JitLimits, JitValueType,
    fuzzing::validate_and_codegen_scalar, validate_function,
};

const MAX_REGS: u32 = 32;
const MAX_GENERATED_INSTRUCTIONS: usize = 256;

fn byte(data: &[u8], cursor: &mut usize) -> u8 {
    let value = data.get(*cursor).copied().unwrap_or(0);
    *cursor = cursor.saturating_add(1);
    value
}

fn reg(data: &[u8], cursor: &mut usize, n_regs: u32) -> u32 {
    u32::from(byte(data, cursor)) % n_regs
}

fn build_function(data: &[u8]) -> JitFunction {
    let mut cursor = 0;
    let n_regs = 1 + u32::from(byte(data, &mut cursor)) % MAX_REGS;
    let n_params = u32::from(byte(data, &mut cursor)) % (n_regs + 1);
    let mut code = Vec::with_capacity(MAX_GENERATED_INSTRUCTIONS + n_regs as usize + 1);

    // Initialize every non-parameter register so a useful proportion of the
    // generated corpus crosses the validation boundary and exercises deeper CFG
    // and data-flow checks. Other byte choices still create invalid control flow
    // and must be rejected without panicking.
    for dst in n_params..n_regs {
        code.push(JitInstr::LoadInt {
            dst,
            value: i64::from(byte(data, &mut cursor) as i8),
        });
    }

    while cursor < data.len() && code.len() < MAX_GENERATED_INSTRUCTIONS {
        let opcode = byte(data, &mut cursor) % 10;
        let dst = reg(data, &mut cursor, n_regs);
        let lhs = reg(data, &mut cursor, n_regs);
        let rhs = reg(data, &mut cursor, n_regs);
        let instruction = match opcode {
            0 => JitInstr::LoadInt {
                dst,
                value: i64::from(byte(data, &mut cursor) as i8),
            },
            1 => JitInstr::Move { dst, src: lhs },
            2 => JitInstr::Add { dst, lhs, rhs },
            3 => JitInstr::Sub { dst, lhs, rhs },
            4 => JitInstr::Mul { dst, lhs, rhs },
            5 => JitInstr::Div { dst, lhs, rhs },
            6 => JitInstr::Mod { dst, lhs, rhs },
            7 => JitInstr::Equal { dst, lhs, rhs },
            8 => JitInstr::Compare {
                dst,
                lhs,
                rhs,
                op: match byte(data, &mut cursor) % 4 {
                    0 => JitCompare::Lt,
                    1 => JitCompare::Le,
                    2 => JitCompare::Gt,
                    _ => JitCompare::Ge,
                },
            },
            _ => JitInstr::Nop,
        };
        code.push(instruction);
    }
    code.push(JitInstr::Return {
        src: reg(data, &mut cursor, n_regs),
    });

    JitFunction {
        n_params,
        n_regs,
        reg_types: vec![JitValueType::Int; n_regs as usize],
        zero_init_regs: Vec::new(),
        code,
        memo_scopes: Vec::new(),
        cold_blocks: Vec::new(),
        resume_live_regs: Vec::new(),
    }
}

fuzz_target!(|data: &[u8]| {
    let function = build_function(data);
    if validate_function(&function).is_ok() {
        // Keep every codegen attempt small even if a future generator raises its
        // local caps. The engine independently revalidates against these limits
        // before allocating executable memory or invoking Cranelift.
        let limits = JitLimits {
            max_instructions: MAX_GENERATED_INSTRUCTIONS + MAX_REGS as usize + 1,
            max_registers: MAX_REGS as usize,
            max_parameters: MAX_REGS as usize,
            max_cfg_edges: 512,
            max_total_operands: 2_048,
            max_analysis_cells: 16_384,
            max_deopt_payload_words: 4_096,
            max_memo_scopes: 0,
            max_memo_slots: 0,
            max_native_callees: 0,
            max_group_members: 0,
            max_ir_work_units: 65_536,
        };
        let _ = validate_and_codegen_scalar(&function, limits);
    }
});
