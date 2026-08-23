// --- IR validation: malformed public IR must fail cleanly, not panic ---

#[test]
fn rejects_out_of_range_register() {
    // `Add` reads register 5 in a 3-register function.
    let err = validate(&f(
        1,
        3,
        vec![JitInstr::Add {
            dst: 0,
            lhs: 5,
            rhs: 1,
        }],
    ))
    .unwrap_err();
    assert!(err.message.contains("out of range"), "{}", err.message);
}

#[test]
fn rejects_out_of_range_jump_target() {
    let err = validate(&f(1, 1, vec![JitInstr::Jump { target: 9 }])).unwrap_err();
    assert!(err.message.contains("target 9"), "{}", err.message);
}

#[test]
fn rejects_out_of_range_cold_block_hint() {
    let mut prog = f(1, 1, vec![JitInstr::Return { src: 0 }]);
    prog.cold_blocks.push(3);
    let err = validate(&prog).unwrap_err();
    assert!(
        err.message.contains("cold block instruction 3"),
        "{}",
        err.message
    );
}

#[cfg(feature = "speculation")]
#[test]
fn compiles_valid_cold_block_hint() {
    let mut m = module();
    let mut prog = ft(
        1,
        vec![JitValueType::Bool],
        vec![
            JitInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 2,
            },
            JitInstr::Return { src: 0 },
            JitInstr::Return { src: 0 },
        ],
    );
    prog.cold_blocks.push(2);
    m.compile(&prog)
        .expect("cold block metadata must be a layout-only hint");
}

#[test]
fn rejects_conditional_branch_without_fallthrough() {
    // A trailing conditional branch has no `i + 1` to fall through to.
    let err = validate(&ft(
        1,
        vec![JitValueType::Bool],
        vec![JitInstr::JumpIfBool {
            cond: 0,
            expected: true,
            target: 0,
        }],
    ))
    .unwrap_err();
    assert!(err.message.contains("fall-through"), "{}", err.message);
}

#[test]
fn rejects_reg_types_length_mismatch() {
    let bad = JitFunction {
        n_params: 0,
        n_regs: 3,
        reg_types: vec![JitValueType::Int; 2],
        zero_init_regs: Vec::new(),
        code: vec![],
        memo_scopes: Vec::new(),
        cold_blocks: Vec::new(),
    };
    let err = validate(&bad).unwrap_err();
    assert!(err.message.contains("reg_types length"), "{}", err.message);
}

#[test]
fn rejects_params_exceeding_regs() {
    let err = validate(&f(4, 2, vec![])).unwrap_err();
    assert!(err.message.contains("n_params"), "{}", err.message);
}

#[test]
fn rejects_excessive_combined_analysis_dimensions() {
    let program = JitFunction {
        n_params: 0,
        n_regs: 1_001,
        reg_types: vec![JitValueType::Int; 1_001],
        zero_init_regs: Vec::new(),
        code: vec![JitInstr::Jump { target: 0 }; 1_000],
        memo_scopes: Vec::new(),
        cold_blocks: Vec::new(),
    };

    let error = validate(&program).expect_err("analysis matrices must have a joint limit");
    assert!(error.message.contains("analysis size"), "{}", error.message);
}

#[test]
fn deterministic_work_budget_rejects_before_expensive_analysis() {
    let program = f(1, 1, vec![JitInstr::Return { src: 0 }]);
    let limits = JitLimits {
        max_ir_work_units: 0,
        ..JitLimits::default()
    };
    let error = ValidatedJitFunction::with_limits(&program, &limits)
        .err()
        .expect("zero work budget must reject even a minimal function");
    assert_eq!(error.kind, JitErrorKind::InvalidIr);
    assert!(error.message.contains("work estimate"), "{}", error.message);
}

#[test]
fn rejects_inconsistent_return_types() {
    use JitValueType::{Bool, Handle, Int};
    let err = validate(&ft(
        1,
        vec![Bool, Int, Handle],
        vec![
            JitInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 3,
            },
            JitInstr::LoadInt { dst: 1, value: 7 },
            JitInstr::Return { src: 1 },
            JitInstr::Return { src: 2 },
        ],
    ))
    .unwrap_err();
    assert!(
        err.message.contains("inconsistent result types"),
        "{}",
        err.message
    );
}

#[cfg(feature = "recursion")]
#[test]
fn rejects_callself_result_type_mismatch() {
    use JitValueType::{Float, Int};
    let err = validate(&ft(
        1,
        vec![Int, Int, Float],
        vec![
            JitInstr::LoadInt { dst: 1, value: 0 },
            JitInstr::CallSelf {
                dst: 2,
                args: vec![1],
            },
            JitInstr::Return { src: 1 },
        ],
    ))
    .expect_err("CallSelf destination must match the function return type");
    assert!(err.message.contains("CallSelf result"), "{}", err.message);
}

#[cfg(feature = "recursion")]
#[test]
fn rejects_callself_flat_parameters_until_lengths_are_supported() {
    use JitValueType::{FlatInt, Int};
    let err = validate(&ft(
        1,
        vec![FlatInt, Int],
        vec![
            JitInstr::CallSelf {
                dst: 1,
                args: vec![0],
            },
            JitInstr::Return { src: 1 },
        ],
    ))
    .expect_err("CallSelf must not silently discard flat lengths");
    assert!(
        err.message.contains("flat-array parameters"),
        "{}",
        err.message
    );
}

#[test]
fn rejects_reachable_use_before_definition() {
    let err = validate(&f(0, 1, vec![JitInstr::Return { src: 0 }]))
        .expect_err("undefined register reads must not become zero");
    assert!(
        err.message.contains("before it is definitely assigned"),
        "{}",
        err.message
    );
}

#[test]
fn rejects_map_match_payload_read_on_none_edge() {
    use JitValueType::{Float, Handle, Int};
    let cases = [
        (
            "map-int",
            Int,
            JitInstr::MatchMapGetInt {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 1,
                none_ip: 2,
            },
        ),
        (
            "map-float",
            Float,
            JitInstr::MatchMapGetFloat {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 1,
                none_ip: 2,
            },
        ),
        (
            "sorted-map-int",
            Int,
            JitInstr::MatchSortedMapGetInt {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 1,
                none_ip: 2,
            },
        ),
        (
            "sorted-map-float",
            Float,
            JitInstr::MatchSortedMapGetFloat {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 1,
                none_ip: 2,
            },
        ),
    ];
    for (name, payload_type, match_instr) in cases {
        let error = validate(&ft(
            2,
            vec![Handle, Int, payload_type],
            vec![
                match_instr,
                JitInstr::Return { src: 2 },
                JitInstr::Return { src: 2 },
            ],
        ))
        .expect_err("None edge must not define the fused match payload");
        assert!(
            error.message.contains("before it is definitely assigned"),
            "{name}: {}",
            error.message
        );
    }

    let error = validate(&ft(
        2,
        vec![Handle, Int, Int],
        vec![
            JitInstr::MatchMapGetInt {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 1,
                none_ip: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    ))
    .expect_err("a shared Some/None successor cannot assume a payload");
    assert!(error.message.contains("before it is definitely assigned"));
}

#[test]
fn permits_explicit_scalar_zero_initialized_scratch() {
    let mut program = f(0, 1, vec![JitInstr::Return { src: 0 }]);
    program.zero_init_regs.push(0);
    validate(&program).expect("declared scalar scratch has defined zero entry value");
}

#[test]
fn rejects_zero_initialized_handle_scratch() {
    let mut program = ft(
        0,
        vec![JitValueType::Handle],
        vec![JitInstr::Return { src: 0 }],
    );
    program.zero_init_regs.push(0);
    let err = validate(&program).expect_err("a zero word is not a valid heap handle");
    assert!(err.message.contains("scalar type"), "{}", err.message);
}

#[cfg(feature = "memoization")]
#[test]
fn rejects_duplicate_or_out_of_range_memo_slots() {
    use JitValueType::{Handle, Int};
    let out_of_range = ft(
        1,
        vec![Handle, Int],
        vec![
            JitInstr::MemoizedHostCall {
                helper: HostHelper::StringLen,
                dst: 1,
                args: vec![HostArg::Reg(0)],
                memo_slot: 1,
            },
            JitInstr::Return { src: 1 },
        ],
    );
    let err = validate(&out_of_range).expect_err("one memo site only has slot zero");
    assert!(err.message.contains("out of range"), "{}", err.message);

    let duplicate = ft(
        1,
        vec![Handle, Int, Int],
        vec![
            JitInstr::MemoizedHostCall {
                helper: HostHelper::StringLen,
                dst: 1,
                args: vec![HostArg::Reg(0)],
                memo_slot: 0,
            },
            JitInstr::MemoizedHostCall {
                helper: HostHelper::StringLen,
                dst: 2,
                args: vec![HostArg::Reg(0)],
                memo_slot: 0,
            },
            JitInstr::Return { src: 2 },
        ],
    );
    let err = validate(&duplicate).expect_err("memoization sites need distinct slots");
    assert!(
        err.message.contains("shared by instructions"),
        "{}",
        err.message
    );
}

#[cfg(feature = "memoization")]
#[test]
fn rejects_handle_returning_memoized_helper() {
    use JitValueType::{Handle, Int};
    let err = validate(&ft(
        1,
        vec![Int, Handle],
        vec![
            JitInstr::MemoizedHostCall {
                helper: HostHelper::StringFromInt,
                dst: 1,
                args: vec![HostArg::Reg(0)],
                memo_slot: 0,
            },
            JitInstr::Return { src: 1 },
        ],
    ))
    .expect_err("memoization is restricted to non-allocating scalar results");
    assert!(
        err.message.contains("result must be a scalar"),
        "{}",
        err.message
    );
}

#[test]
fn rejects_bool_arithmetic_and_accepts_float_compare_branches() {
    use JitValueType::{Bool, Float, Int};
    let err = validate(&ft(
        2,
        vec![Bool, Bool, Bool],
        vec![
            JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::Return { src: 2 },
        ],
    ))
    .expect_err("Bool arithmetic is not numeric");
    assert!(err.message.contains("Int or Float"), "{}", err.message);

    validate(&ft(
        2,
        vec![Float, Float],
        vec![
            JitInstr::JumpIfIntCompare {
                op: JitCompare::Lt,
                lhs: 0,
                rhs: 1,
                expected: true,
                target: 1,
            },
            JitInstr::Return { src: 0 },
        ],
    ))
    .expect("comparison branches accept same-class Float operands");

    #[cfg(feature = "speculation")]
    validate(&ft(
        2,
        vec![Float, Float],
        vec![
            JitInstr::ProfiledJumpIfIntCompare {
                op: JitCompare::Lt,
                lhs: 0,
                rhs: 1,
                expected: true,
                target: 1,
                hot_target: true,
            },
            JitInstr::Return { src: 0 },
        ],
    ))
    .expect("profiled comparison branches accept same-class Float operands");

    let err = validate(&ft(
        2,
        vec![Float, Int],
        vec![
            JitInstr::JumpIfIntCompare {
                op: JitCompare::Lt,
                lhs: 0,
                rhs: 1,
                expected: true,
                target: 1,
            },
            JitInstr::Return { src: 0 },
        ],
    ))
    .expect_err("comparison branches reject mixed numeric classes");
    assert!(err.message.contains("classes differ"), "{}", err.message);
}

#[cfg(feature = "recursion")]
#[test]
fn canonical_leaf_classifier_covers_recursive_and_float_conversion_ops() {
    use JitValueType::{Float, Int};

    let float_to_int = ft(
        1,
        vec![Float, Int],
        vec![
            JitInstr::FloatToInt {
                dst: 1,
                src: 0,
                rounding: FloatRounding::Floor,
            },
            JitInstr::Return { src: 1 },
        ],
    );
    assert!(is_native_callable_leaf(&float_to_int));

    let guarded = f(
        1,
        1,
        vec![
            JitInstr::TailCallGuard { max_depth: 32 },
            JitInstr::Return { src: 0 },
        ],
    );
    assert!(is_native_callable_leaf(&guarded));

    let grouped = f(
        1,
        2,
        vec![
            JitInstr::CallGroup {
                group_index: 0,
                dst: 1,
                args: vec![0],
            },
            JitInstr::Return { src: 1 },
        ],
    );
    assert!(is_native_callable_leaf(&grouped));
}

#[cfg(feature = "recursion")]
#[test]
fn recursive_stack_cap_never_exceeds_estimated_budget() {
    let args = (0..4096).collect::<Vec<_>>();
    let program = f(
        4096,
        4097,
        vec![
            JitInstr::CallSelf { dst: 4096, args },
            JitInstr::Return { src: 4096 },
        ],
    );
    let frame = native_recursion_frame_bytes_estimate(&program);
    let cap = native_recursion_depth_cap(&program);
    assert!(cap >= 0);
    assert!(
        frame.saturating_mul(cap) <= NATIVE_RECURSION_STACK_BUDGET_BYTES,
        "frame={frame} cap={cap}"
    );
}

#[test]
fn rejects_mutable_flat_returns() {
    for ty in [JitValueType::FlatIntMut, JitValueType::FlatFloatMut] {
        let err = validate(&ft(1, vec![ty], vec![JitInstr::Return { src: 0 }])).unwrap_err();
        assert!(err.message.contains("flat-array"), "{}", err.message);
    }
}

#[test]
fn tail_call_guard_uses_embedding_logical_depth() {
    let mut module = module();
    let function = ft(
        0,
        vec![JitValueType::Int],
        vec![
            JitInstr::TailCallGuard { max_depth: 100 },
            JitInstr::LoadInt { dst: 0, value: 7 },
            JitInstr::Return { src: 0 },
        ],
    );
    let id = module.compile(&function).expect("compile");
    assert_eq!(
        module.call_with_host_ctx_at_depth(
            id,
            &[],
            &[],
            0,
            &mut [],
            LogicalCallDepth {
                current: 1,
                limit: 2,
            },
        ),
        NativeOutcome::Completed(7)
    );
    assert!(matches!(
        module.call_with_host_ctx_at_depth(
            id,
            &[],
            &[],
            0,
            &mut [],
            LogicalCallDepth {
                current: 2,
                limit: 2,
            },
        ),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId::ANONYMOUS,
            ..
        }
    ));
}

#[test]
fn osr_exit_returns_updated_logical_tail_depth() {
    let mut module = module();
    let function = ft(
        0,
        vec![JitValueType::Int],
        vec![
            JitInstr::TailCallGuard { max_depth: 10_000 },
            JitInstr::LoadInt { dst: 0, value: 7 },
            JitInstr::OsrExit,
        ],
    );
    let id = module
        .compile_osr(&function, 0, false, false)
        .expect("compile OSR function");
    let outcome = module.call_with_host_ctx_at_depth(
        id,
        &[0],
        &[0],
        0,
        &mut [],
        LogicalCallDepth {
            current: 501,
            limit: 1_000,
        },
    );
    assert!(
        matches!(
            &outcome,
            NativeOutcome::Deopt {
                logical_depth: Some(502),
                ..
            }
        ),
        "{outcome:?}"
    );
}

#[test]
fn enforces_normal_and_osr_terminators() {
    let normal = f(0, 0, vec![JitInstr::OsrExit]);
    assert!(crate::validate(&normal, false).is_err());

    let osr = f(1, 1, vec![JitInstr::Return { src: 0 }]);
    assert!(crate::validate(&osr, true).is_err());
}

#[test]
fn rejects_int_op_on_float_register() {
    use JitValueType::{Float, Int};
    // `Mod` (integer-only) applied to float registers.
    let err = validate(&ft(
        2,
        vec![Float, Float, Int],
        vec![JitInstr::Mod {
            dst: 2,
            lhs: 0,
            rhs: 1,
        }],
    ))
    .unwrap_err();
    assert!(err.message.contains("must be Int"), "{}", err.message);
}

#[test]
fn rejects_mismatched_arith_classes() {
    use JitValueType::{Float, Int};
    // `Add` with one int and one float operand.
    let err = validate(&ft(
        2,
        vec![Int, Float, Int],
        vec![JitInstr::Add {
            dst: 2,
            lhs: 0,
            rhs: 1,
        }],
    ))
    .unwrap_err();
    assert!(err.message.contains("classes differ"), "{}", err.message);
}

#[test]
fn rejects_handle_outside_heap_read_base() {
    use JitValueType::{Handle, Int};
    // A `Handle` register used as an arithmetic operand.
    let err = validate(&ft(
        2,
        vec![Handle, Int, Int],
        vec![JitInstr::Add {
            dst: 2,
            lhs: 0,
            rhs: 1,
        }],
    ))
    .unwrap_err();
    assert!(err.message.contains("Handle"), "{}", err.message);
}

#[test]
fn rejects_non_handle_heap_read_base() {
    // `FieldInt` host helper base must be a `Handle`, not an `Int`.
    let err = validate(&f(
        1,
        2,
        vec![JitInstr::HostCall {
            helper: HostHelper::FieldInt,
            dst: 1,
            args: vec![HostArg::Reg(0), HostArg::ImmI64(0)],
        }],
    ))
    .unwrap_err();
    assert!(err.message.contains("expected Handle"), "{}", err.message);
}

#[test]
fn accepts_well_formed_heap_read() {
    use JitValueType::{Handle, Int};
    validate(&ft(
        1,
        vec![Handle, Int],
        vec![
            JitInstr::HostCall {
                helper: HostHelper::ListLen,
                dst: 1,
                args: vec![HostArg::Reg(0)],
            },
            JitInstr::Return { src: 1 },
        ],
    ))
    .expect("well-formed heap read should validate");
}

// fn(a, b) { return a + b } — a 2-param function for the call-guard tests.
pub(super) fn two_param_add() -> JitFunction {
    f(
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
    )
}

#[test]
fn call_rejects_wrong_arg_count() {
    // The generated entry block reads exactly `n_params` words from `args_ptr`,
    // so a short slice must be rejected by `call` (otherwise: out-of-bounds
    // read). Both too-few and too-many fall back rather than misread memory.
    let mut m = module();
    let id = m.compile(&two_param_add()).unwrap();
    assert_eq!(m.callt(id, &[2, 3]), Some(5));
    assert_eq!(m.callt(id, &[2]), None); // too few — must not read past the slice
    assert_eq!(m.callt(id, &[]), None);
    assert_eq!(m.callt(id, &[2, 3, 4]), None); // too many
}

#[test]
fn safe_call_rejects_forged_flat_pointer() {
    use JitValueType::{FlatInt, Int};
    let mut m = module();
    let id = m
        .compile(&ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    assert!(matches!(
        m.call(id, &[1], &[100]),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId::ANONYMOUS,
            ..
        }
    ));
}

#[test]
fn armed_osr_rejects_the_ordinary_call_mode() {
    let mut m = module();
    let code = vec![
        JitInstr::LoadInt { dst: 1, value: 1 },
        JitInstr::JumpIfIntCompare {
            lhs: 0,
            rhs: 1,
            op: JitCompare::Gt,
            expected: false,
            target: 4,
        },
        JitInstr::Sub {
            dst: 0,
            lhs: 0,
            rhs: 1,
        },
        JitInstr::Jump { target: 1 },
        JitInstr::OsrExit,
    ];
    let id = m.compile_osr(&f(1, 2, code), 1, true, false).unwrap();
    let window = [3, 1];
    let lens = [0; 2];
    assert!(matches!(
        m.call(id, &window, &lens),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId::ANONYMOUS,
            ..
        }
    ));
    let mut limits = [0, 100, 0];
    // SAFETY: this test supplies the required live limits cell and no raw flat
    // arguments. The purpose is to prove the matching raw mode remains usable.
    let outcome = unsafe { m.call_with_limits(id, &window, &lens, 0, limits.as_mut_ptr()) };
    assert!(
        matches!(outcome, NativeOutcome::Deopt { safepoint_id, .. } if safepoint_id != SafepointId::ANONYMOUS)
    );
}

#[test]
fn cancel_uses_atomic_load() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let mut m = module();
    let code = vec![
        JitInstr::LoadInt { dst: 1, value: 1 },
        JitInstr::JumpIfIntCompare {
            lhs: 0,
            rhs: 1,
            op: JitCompare::Gt,
            expected: false,
            target: 4,
        },
        JitInstr::Sub {
            dst: 0,
            lhs: 0,
            rhs: 1,
        },
        JitInstr::Jump { target: 1 },
        JitInstr::OsrExit,
    ];
    let id = m.compile_osr(&f(1, 2, code), 1, false, true).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = Arc::clone(&cancel);
    let thread = std::thread::spawn(move || setter.store(true, Ordering::Relaxed));
    let window = [1_000_000_000, 1];
    let lens = [0; 2];
    let mut limits = [0, -1, Arc::as_ptr(&cancel) as i64];
    // SAFETY: the AtomicBool and limits cell remain live through the native
    // call, and this program has no flat arguments.
    let outcome = unsafe { m.call_with_limits(id, &window, &lens, 0, limits.as_mut_ptr()) };
    thread.join().unwrap();
    assert!(
        matches!(outcome, NativeOutcome::Deopt { safepoint_id, .. } if safepoint_id != SafepointId::ANONYMOUS)
    );
}

#[test]
fn precise_deopt_preserves_bool_logical_type() {
    use JitValueType::{Bool, Int};
    let mut m = module();
    let id = m
        .compile(&ft(
            3,
            vec![Bool, Int, Int, Bool, Int],
            vec![
                JitInstr::LoadBool {
                    dst: 3,
                    value: true,
                },
                JitInstr::Add {
                    dst: 4,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let NativeOutcome::Deopt { live, .. } = m.call(id, &[0, i64::MAX, 1], &[0; 3]) else {
        panic!("overflow must deopt");
    };
    assert!(
        live.iter()
            .any(|reg| reg.reg == 0 && reg.value == DeoptValue::Bool(false))
    );
    assert!(
        live.iter()
            .any(|reg| reg.reg == 3 && reg.value == DeoptValue::Bool(true))
    );
}

#[cfg(feature = "recursion")]
#[test]
fn recursive_group_rejects_external_native_call_before_declaration() {
    use JitValueType::Int;
    let mut m = module();
    let leaf = m
        .compile(&ft(1, vec![Int], vec![JitInstr::Return { src: 0 }]))
        .unwrap();
    let member = ft(
        1,
        vec![Int, Int],
        vec![
            JitInstr::CallNative {
                callee: leaf,
                dst: 1,
                args: vec![0],
            },
            JitInstr::Return { src: 1 },
        ],
    );
    let err = m.compile_recursive_group(&[member]).unwrap_err();
    assert!(
        err.message.contains("unsupported CallNative"),
        "{}",
        err.message
    );
    let after = m
        .compile(&ft(1, vec![Int], vec![JitInstr::Return { src: 0 }]))
        .expect("preflight rejection must not poison the module");
    assert_eq!(m.call(after, &[9], &[0]).completed(), Some(9));
}

#[test]
fn deep_acyclic_native_call_chain_deopts_at_cap() {
    use JitValueType::Int;
    let mut m = module();
    let mut callee = m
        .compile(&ft(1, vec![Int], vec![JitInstr::Return { src: 0 }]))
        .unwrap();
    for _ in 0..300 {
        callee = m
            .compile(&ft(
                1,
                vec![Int, Int],
                vec![
                    JitInstr::CallNative {
                        callee,
                        dst: 1,
                        args: vec![0],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
    }
    assert!(matches!(
        m.call(callee, &[7], &[0]),
        NativeOutcome::Deopt { .. }
    ));
}

#[test]
fn call_native_helper_bail_chains_child() {
    use JitValueType::{Handle, Int};
    extern "C" fn bailing_field(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        signal_bail(_ctx);
        0
    }
    let mut m = NativeModule::new(HostHelpers {
        field_int: bailing_field,
        ..host_helpers()
    })
    .unwrap();
    let child = m
        .compile(&ft(
            1,
            vec![Handle, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::FieldInt,
                    dst: 1,
                    args: vec![HostArg::Reg(0), HostArg::ImmI64(0)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let parent = m
        .compile(&ft(
            1,
            vec![Handle, Int],
            vec![
                JitInstr::CallNative {
                    callee: child,
                    dst: 1,
                    args: vec![0],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let NativeOutcome::Deopt { child, .. } = m.call(parent, &[0], &[0]) else {
        panic!("helper bail must deopt parent");
    };
    let child = child.expect("child frame must be retained");
    assert_ne!(child.safepoint_id, SafepointId::ANONYMOUS);
}

#[test]
fn reentrant_host_helper_is_rejected_without_corrupting_outer_call() {
    use JitValueType::{Handle, Int};
    std::thread_local! {
        static TARGET: std::cell::Cell<Option<(*const NativeModule, CompiledId)>> =
            const { std::cell::Cell::new(None) };
    }
    extern "C" fn reenter(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        TARGET.with(|target| {
            let (module, id) = target.get().expect("reentry target installed");
            // SAFETY: the test keeps the module in place and alive across the
            // outer call. The nested safe call must be rejected by the guard.
            let outcome = unsafe { (&*module).call(id, &[7], &[0]) };
            assert!(matches!(
                outcome,
                NativeOutcome::Deopt {
                    safepoint_id: SafepointId::ANONYMOUS,
                    decline: Some(NativeDeclineReason::ReentrantCall),
                    ..
                }
            ));
        });
        42
    }
    let mut m = NativeModule::new(HostHelpers {
        field_int: reenter,
        ..host_helpers()
    })
    .unwrap();
    let nested = m
        .compile(&ft(1, vec![Int], vec![JitInstr::Return { src: 0 }]))
        .unwrap();
    let outer = m
        .compile(&ft(
            1,
            vec![Handle, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::FieldInt,
                    dst: 1,
                    args: vec![HostArg::Reg(0), HostArg::ImmI64(0)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    TARGET.with(|target| target.set(Some((&m, nested))));
    assert_eq!(m.call(outer, &[0], &[0]).completed(), Some(42));
    TARGET.with(|target| target.set(None));
}

#[test]
fn call_rejects_id_from_another_module() {
    // A `CompiledId` minted by one module indexes that module's table; using it
    // against another module must be rejected, not silently mis-dispatched.
    let mut m1 = module();
    let mut m2 = module();
    let id1 = m1.compile(&two_param_add()).unwrap();
    let _id2 = m2.compile(&two_param_add()).unwrap();
    assert_eq!(m1.callt(id1, &[2, 3]), Some(5));
    assert_eq!(m2.callt(id1, &[2, 3]), None); // foreign id → fallback, no panic
}
use super::*;
