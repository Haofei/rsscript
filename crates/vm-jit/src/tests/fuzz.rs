    // --- Structured fuzz: validate/compile robustness (execution spec §7) ------
    //
    // The contract is that `compile` is *total* over arbitrary `JitFunction`
    // values: a producer bug (out-of-range register, type-mismatched operand,
    // wild jump target, truncated stream) MUST surface as a clean `JitError` —
    // never a panic, never undefined behaviour, never silently-wrong machine code.
    // These tests drive thousands of random and mutation-derived programs through
    // `compile` (which runs `validate` then Cranelift codegen) and assert it
    // always returns (`Ok` or `Err`). Miscompile detection is the differential
    // suite's job (compile-vs-interpreter on real programs); here we only pin
    // robustness. Randomness is a fixed-seed xorshift so failures are reproducible
    // without an external rng/proptest dependency.

    /// Deterministic xorshift64* PRNG — reproducible, no external dep.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u32) -> u32 {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as u32
            }
        }
        /// A register index: usually in `0..n_regs`, occasionally out of range so
        /// `validate`'s bounds checks are exercised.
        fn reg(&mut self, n_regs: u32) -> u32 {
            if self.next() & 7 == 0 {
                self.below(n_regs.saturating_mul(2).saturating_add(3))
            } else {
                self.below(n_regs.max(1))
            }
        }
        fn vty(&mut self) -> JitValueType {
            match self.below(5) {
                0 => JitValueType::Int,
                1 => JitValueType::Float,
                2 => JitValueType::FlatInt,
                3 => JitValueType::FlatFloat,
                _ => JitValueType::Handle,
            }
        }
    }

    /// One random instruction. `n` is the code length (for jump targets), which
    /// may be exceeded so out-of-range targets are tested too.
    fn random_instr(rng: &mut Rng, n_regs: u32, n: u32) -> JitInstr {
        let r = |rng: &mut Rng| rng.reg(n_regs);
        let t = |rng: &mut Rng| rng.below(n.saturating_add(2));
        match rng.below(31) {
            22 => JitInstr::HostCall {
                helper: HostHelper::FieldFloat,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            23 => JitInstr::HostCall {
                helper: HostHelper::ListGetFloat,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng)), HostArg::Reg(r(rng))],
            },
            24 => JitInstr::ListGetIntDirect {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            25 => JitInstr::ListGetFloatDirect {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            26 => JitInstr::ListLenDirect {
                dst: r(rng),
                base: r(rng),
            },
            0 => JitInstr::Nop,
            1 => JitInstr::Bail,
            2 => JitInstr::LoadInt {
                dst: r(rng),
                value: rng.next() as i64,
            },
            3 => JitInstr::LoadFloat {
                dst: r(rng),
                value: f64::from_bits(rng.next()),
            },
            4 => JitInstr::LoadBool {
                dst: r(rng),
                value: rng.next() & 1 == 0,
            },
            5 => JitInstr::Move {
                dst: r(rng),
                src: r(rng),
            },
            6 => JitInstr::Add {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            7 => JitInstr::Sub {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            8 => JitInstr::Mul {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            9 => JitInstr::Div {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            10 => JitInstr::Mod {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            11 => JitInstr::BitAnd {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            12 => JitInstr::Shl {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            13 => JitInstr::Shr {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            14 => JitInstr::Equal {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            15 => JitInstr::Compare {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
                op: match rng.below(4) {
                    0 => JitCompare::Lt,
                    1 => JitCompare::Le,
                    2 => JitCompare::Gt,
                    _ => JitCompare::Ge,
                },
            },
            16 => JitInstr::Jump { target: t(rng) },
            17 => JitInstr::JumpIfBool {
                cond: r(rng),
                expected: rng.next() & 1 == 0,
                target: t(rng),
            },
            18 => JitInstr::Return { src: r(rng) },
            19 => JitInstr::HostCall {
                helper: HostHelper::FieldInt,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            20 => JitInstr::HostCall {
                helper: HostHelper::ListLen,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng))],
            },
            21 => JitInstr::HostCall {
                helper: HostHelper::ListGetInt,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng)), HostArg::Reg(r(rng))],
            },
            27 => JitInstr::HostCall {
                helper: HostHelper::ClosureId,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng))],
            },
            28 => JitInstr::HostCall {
                helper: HostHelper::ClosureCapture,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            29 => JitInstr::HostCall {
                helper: HostHelper::FieldHandle,
                dst: r(rng),
                args: vec![
                    HostArg::Reg(r(rng)),
                    HostArg::ImmI64(i64::from(rng.below(8))),
                ],
            },
            _ => JitInstr::HostCall {
                helper: HostHelper::ListGetHandle,
                dst: r(rng),
                args: vec![HostArg::Reg(r(rng)), HostArg::Reg(r(rng))],
            },
        }
    }

    fn random_program(rng: &mut Rng) -> JitFunction {
        let n_regs = rng.below(6); // 0..=5, includes the empty-window edge case
        let n_params = if n_regs == 0 {
            0
        } else {
            rng.below(n_regs + 1)
        };
        let len = rng.below(14);
        let reg_types = (0..n_regs).map(|_| rng.vty()).collect();
        let code = (0..len).map(|_| random_instr(rng, n_regs, len)).collect();
        JitFunction {
            n_params,
            n_regs,
            reg_types,
            zero_init_regs: Vec::new(),
            code,
            memo_scopes: Vec::new(),
            cold_blocks: Vec::new(),
        }
    }

    #[test]
    fn fuzz_compile_is_total_over_arbitrary_ir() {
        let mut m = module();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..6000 {
            let prog = random_program(&mut rng);
            // The whole contract: never panic. Both arms are acceptable.
            match m.compile(&prog) {
                Ok(_) | Err(_) => {}
            }
        }
    }

    #[test]
    fn fuzz_compile_is_total_over_mutated_valid_ir() {
        // Seed: `fn(a, b) { t = a + b; return t }` — a known-valid program. Each
        // round perturbs one field (opcode swap, register bump, target bump,
        // truncation) and re-compiles; a mutation that invalidates the IR must be
        // caught as a clean error, not a panic.
        let mut m = module();
        let mut rng = Rng(0x1234_5678_9ABC_DEF0);
        let base = f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        for _ in 0..4000 {
            let mut prog = base.clone();
            match rng.below(5) {
                0 => prog.n_regs = rng.below(6),
                1 => prog.n_params = rng.below(6),
                2 => {
                    if !prog.code.is_empty() {
                        let idx = rng.below(prog.code.len() as u32) as usize;
                        prog.code[idx] =
                            random_instr(&mut rng, prog.n_regs.max(1), prog.code.len() as u32);
                    }
                }
                3 => prog
                    .code
                    .truncate(rng.below(prog.code.len() as u32 + 1) as usize),
                _ => {
                    if !prog.reg_types.is_empty() {
                        let idx = rng.below(prog.reg_types.len() as u32) as usize;
                        prog.reg_types[idx] = rng.vty();
                    }
                }
            }
            match m.compile(&prog) {
                Ok(_) | Err(_) => {}
            }
        }
    }

    /// Execution robustness + host-helper handle fuzz: drive *loop-free* (forward-
    /// jump-only, so guaranteed-terminating) validated programs through `call` with
    /// random argument bit patterns — including `Handle` args fed to the no-op
    /// `field_int`/`list_len`/`list_get_int` helpers at random slots/indices. The
    /// compiled code must always return cleanly (`Some`/`None` — a value or a bail),
    /// never UB or a hang. Loop-free generation is what keeps this from spinning on
    /// the native tier, which (by design, §6.2) has no internal step limit.
    #[test]
    fn fuzz_straightline_execution_never_traps_host() {
        let mut m = module();
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..3000 {
            let n_regs = rng.below(5).max(1);
            let n_params = rng.below(n_regs + 1);
            let len = rng.below(8);
            let reg_types: Vec<JitValueType> = (0..n_regs).map(|_| rng.vty()).collect();
            let mut code = Vec::new();
            for i in 0..len {
                // Forward-only jumps (target strictly after this index, up to `len`),
                // so control flow always makes progress to the end.
                let forward = i + 1 + rng.below(len.saturating_sub(i).max(1));
                let instr = match rng.below(12) {
                    0 => JitInstr::Jump { target: forward },
                    1 => JitInstr::JumpIfBool {
                        cond: rng.below(n_regs),
                        expected: rng.next() & 1 == 0,
                        target: forward,
                    },
                    other => random_instr(&mut rng, n_regs, len).pipe_nonjump(other),
                };
                code.push(instr);
            }
            // Guarantee a terminating tail so a validated function returns.
            code.push(JitInstr::Return {
                src: rng.below(n_regs),
            });
            let prog = JitFunction {
                n_params,
                n_regs,
                reg_types,
                zero_init_regs: Vec::new(),
                code,
                memo_scopes: Vec::new(),
                cold_blocks: Vec::new(),
            };
            if let Ok(id) = m.compile(&prog) {
                let args: Vec<i64> = (0..n_params).map(|_| rng.next() as i64).collect();
                // Must return without UB/hang; value or bail are both fine.
                let _ = m.callt(id, &args);
            }
        }
    }

    // Small helper: keep a non-jump instruction as-is (jumps are generated with
    // forward targets separately above).
    impl JitInstr {
        fn pipe_nonjump(self, _tag: u32) -> JitInstr {
            match self {
                // Re-point any stray jump the generator produced to a Nop so this
                // path stays loop-free; all other instructions pass through.
                JitInstr::Jump { .. } | JitInstr::JumpIfBool { .. } => JitInstr::Nop,
                other => other,
            }
        }
    }

