#[test]
fn compiles_and_runs_float_arith() {
    use JitValueType::{Float, Int};
    let mut m = module();
    // fn(a: f64, b: f64) -> f64 { return a * b - a }  regs 0=a,1=b,2=t
    let id = m
        .compile(&ft(
            2,
            vec![Float, Float, Float],
            vec![
                JitInstr::Mul {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Sub {
                    dst: 2,
                    lhs: 2,
                    rhs: 0,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert!(
        !m.has_direct_scalar_entry(id),
        "ordinary VM entries must not duplicate code for an unused direct ABI"
    );
    let call = |a: f64, b: f64| {
        f64::from_bits(
            m.callt(id, &[a.to_bits() as i64, b.to_bits() as i64])
                .unwrap() as u64,
        )
    };
    assert_eq!(call(2.5, 4.0), 2.5 * 4.0 - 2.5);
    assert_eq!(call(3.0, 0.0), -3.0);
    let _ = Int; // silence unused in case
}

#[test]
fn native_scalar_call_invokes_compiled_int_leaf() {
    let mut m = module();
    // callee add2(a, b) = a + b
    let callee = m
        .compile(&f(
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
        ))
        .unwrap();
    assert!(
        m.compact_scalar_frame_callable(callee),
        "helper-free scalar leaves should omit the child lens window"
    );
    assert!(
        !m.has_direct_scalar_entry(callee),
        "checked integer addition must retain the precise-deopt child ABI"
    );
    // caller(x) = add2(x, 7) * 2
    let caller = m
        .compile(&f(
            1,
            5,
            vec![
                JitInstr::LoadInt { dst: 1, value: 7 },
                JitInstr::CallNative {
                    callee,
                    dst: 2,
                    args: vec![0, 1],
                },
                JitInstr::LoadInt { dst: 3, value: 2 },
                JitInstr::Mul {
                    dst: 4,
                    lhs: 2,
                    rhs: 3,
                },
                JitInstr::Return { src: 4 },
            ],
        ))
        .unwrap();

    assert_eq!(m.callt(caller, &[5]), Some(24));
    assert_eq!(m.callt(caller, &[-10]), Some(-6));
    assert_eq!(m.direct_scalar_call_edges(caller), Some(0));
}

#[test]
fn compact_scalar_child_frame_preserves_precise_nested_deopt() {
    let mut m = module();
    let callee = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::Div {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert!(!m.has_direct_scalar_entry(callee));
    assert!(m.compact_scalar_frame_callable(callee));
    let caller = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::CallNative {
                    callee,
                    dst: 2,
                    args: vec![0, 1],
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert_eq!(m.direct_scalar_call_edges(caller), Some(0));

    assert_eq!(m.callt(caller, &[12, 3]), Some(4));
    let outcome = m.call(caller, &[12, 0], &[0, 0]);
    assert!(matches!(
        outcome,
        NativeOutcome::Deopt {
            safepoint_id,
            child: Some(_),
            ..
        } if safepoint_id != SafepointId::ANONYMOUS
    ));
}

#[test]
fn compile_records_machine_code_size() {
    let mut m = module();
    let id = m
        .compile(&f(
            1,
            2,
            vec![
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 1,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    assert!(
        m.code_size_bytes(id).is_some_and(|bytes| bytes > 0),
        "compiled functions should expose nonzero emitted machine-code size"
    );

    assert_eq!(
        m.code_size_bytes(CompiledId {
            module_id: id.module_id.wrapping_add(1),
            index: id.index,
        }),
        None,
        "compiled ids from another module must not expose local code metadata"
    );
}

#[test]
fn executable_memory_budget_is_hard_shared_and_released_on_drop() {
    const ARENA_BYTES: u64 = 64 * 1024;
    let budget = ExecutableMemoryBudget::new(ARENA_BYTES * 2);
    {
        let mut first = NativeModule::new_with_opt_and_memory_budget(
            host_helpers(),
            false,
            budget.clone(),
            ARENA_BYTES,
        )
        .unwrap();
        first
            .compile(&f(
                0,
                1,
                vec![
                    JitInstr::LoadInt { dst: 0, value: 1 },
                    JitInstr::Return { src: 0 },
                ],
            ))
            .unwrap();
        assert_eq!(budget.allocated(), ARENA_BYTES);

        let second = NativeModule::new_with_opt_and_memory_budget(
            host_helpers(),
            true,
            budget.clone(),
            ARENA_BYTES,
        )
        .unwrap();
        assert_eq!(budget.allocated(), ARENA_BYTES * 2);

        let error = match NativeModule::new_with_opt_and_memory_budget(
            host_helpers(),
            false,
            budget.clone(),
            ARENA_BYTES,
        ) {
            Ok(_) => panic!("a third module cannot reserve beyond the shared hard budget"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("executable-memory budget exceeded")
        );
        assert_eq!(
            budget.allocated(),
            ARENA_BYTES * 2,
            "a rejected allocation must roll its reservation back"
        );
        drop(second);
        assert_eq!(budget.allocated(), ARENA_BYTES);
    }
    assert_eq!(
        budget.allocated(),
        0,
        "dropping modules must unmap code and return their shared budget"
    );
}

#[test]
fn failed_compile_cannot_grow_fixed_arena_and_drop_releases_it() {
    const ARENA_BYTES: u64 = 64 * 1024;
    let budget = ExecutableMemoryBudget::new(ARENA_BYTES);
    {
        let mut module = NativeModule::new_with_opt_and_memory_budget(
            host_helpers(),
            true,
            budget.clone(),
            ARENA_BYTES,
        )
        .unwrap();
        let mut code = Vec::with_capacity(10_002);
        code.push(JitInstr::LoadInt { dst: 1, value: 1 });
        for _ in 0..10_000 {
            code.push(JitInstr::Add {
                dst: 0,
                lhs: 0,
                rhs: 1,
            });
        }
        code.push(JitInstr::Return { src: 0 });
        module
            .compile(&f(1, 2, code))
            .expect_err("machine code larger than the fixed arena must be rejected");
        assert_eq!(
            budget.allocated(),
            ARENA_BYTES,
            "failed codegen cannot allocate beyond the module's fixed arena"
        );
    }
    assert_eq!(budget.allocated(), 0);
}

#[test]
fn native_scalar_call_invokes_compiled_call_chain() {
    let mut m = module();
    let inc = m
        .compile(&f(
            1,
            3,
            vec![
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert_eq!(m.native_call_depth(inc), Some(0));
    let twice = m
        .compile(&f(
            1,
            4,
            vec![
                JitInstr::CallNative {
                    callee: inc,
                    dst: 1,
                    args: vec![0],
                },
                JitInstr::CallNative {
                    callee: inc,
                    dst: 2,
                    args: vec![0],
                },
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    assert_eq!(m.native_call_depth(twice), Some(1));
    let top = m
        .compile(&f(
            1,
            4,
            vec![
                JitInstr::CallNative {
                    callee: twice,
                    dst: 1,
                    args: vec![0],
                },
                JitInstr::CallNative {
                    callee: twice,
                    dst: 2,
                    args: vec![0],
                },
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();

    assert_eq!(m.native_call_depth(top), Some(2));
    assert_eq!(m.callt(top, &[10]), Some(44));
}

#[test]
fn native_scalar_call_invokes_compiled_float_leaf() {
    use JitValueType::{Float, Int};
    let mut m = module();
    // callee(a: Float, b: Float) = a * b
    let callee = m
        .compile_native_callee(&ft(
            2,
            vec![Float, Float, Float],
            vec![
                JitInstr::Mul {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    assert!(m.has_direct_scalar_entry(callee));
    // caller(x: Float) = callee(x, 4.0) - x
    let caller = m
        .compile(&ft(
            1,
            vec![Float, Float, Float, Float],
            vec![
                JitInstr::LoadFloat { dst: 1, value: 4.0 },
                JitInstr::CallNative {
                    callee,
                    dst: 2,
                    args: vec![0, 1],
                },
                JitInstr::Sub {
                    dst: 3,
                    lhs: 2,
                    rhs: 0,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    assert_eq!(m.direct_scalar_call_edges(caller), Some(1));
    let got = m
        .callt(caller, &[2.5f64.to_bits() as i64])
        .map(|bits| f64::from_bits(bits as u64));
    assert_eq!(got, Some(7.5));
    let _ = Int;
}

#[test]
fn native_call_can_return_child_heap_handle_to_parent() {
    use JitValueType::{Handle, Int};
    extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
        value + 10
    }
    let mut m = NativeModule::new(HostHelpers {
        string_from_int,
        ..host_helpers()
    })
    .unwrap();

    let child = m
        .compile(&ft(
            1,
            vec![Int, Handle],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::StringFromInt,
                    dst: 1,
                    args: vec![HostArg::Reg(0)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let parent = m
        .compile(&ft(
            1,
            vec![Int, Handle],
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

    match m.call(parent, &[37], &[0]) {
        NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 47),
        other => panic!("expected CompletedHandle, got {other:?}"),
    }
}

#[test]
fn native_call_child_heap_handle_can_feed_parent_host_helper() {
    use JitValueType::{Handle, Int};
    extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
        value + 10
    }
    extern "C" fn string_len(_ctx: HostCtx, handle: i64) -> i64 {
        handle * 2
    }
    let mut m = NativeModule::new(HostHelpers {
        string_from_int,
        string_len,
        ..host_helpers()
    })
    .unwrap();

    let child = m
        .compile(&ft(
            1,
            vec![Int, Handle],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::StringFromInt,
                    dst: 1,
                    args: vec![HostArg::Reg(0)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let parent = m
        .compile(&ft(
            1,
            vec![Int, Handle, Int],
            vec![
                JitInstr::CallNative {
                    callee: child,
                    dst: 1,
                    args: vec![0],
                },
                JitInstr::HostCall {
                    helper: HostHelper::StringLen,
                    dst: 2,
                    args: vec![HostArg::Reg(1)],
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();

    match m.call(parent, &[5], &[0]) {
        NativeOutcome::Completed(value) => assert_eq!(value, 30),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
fn native_scalar_call_deopts_at_caller_when_callee_deopts() {
    let mut m = module();
    // callee(a, b) = a + b; overflows for (MAX, 1).
    let callee = m
        .compile(&f(
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
        ))
        .unwrap();
    let caller = m
        .compile(&f(
            2,
            3,
            vec![
                JitInstr::CallNative {
                    callee,
                    dst: 2,
                    args: vec![0, 1],
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();

    assert_eq!(m.callt(caller, &[10, 20]), Some(30));
    assert!(matches!(
        m.call(caller, &[i64::MAX, 1], &[0, 0]),
        NativeOutcome::Deopt {
            safepoint_id: SafepointId(1),
            child: Some(_),
            ..
        }
    ));
}

#[cfg(feature = "recursion")]
#[test]
fn native_self_recursion_computes_and_caps_depth() {
    // sum(n) = if n <= 0 { 0 } else { n + sum(n - 1) }, compiled with CallSelf.
    // reg 0 = n (param); regs 1..3 scratch.
    let mut m = module();
    let sum = m
        .compile(&f(
            1,
            4,
            vec![
                JitInstr::LoadInt { dst: 1, value: 0 },
                JitInstr::JumpIfIntCompare {
                    lhs: 0,
                    rhs: 1,
                    op: JitCompare::Le,
                    expected: true,
                    target: 7,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Sub {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::CallSelf {
                    dst: 3,
                    args: vec![2],
                },
                JitInstr::Add {
                    dst: 3,
                    lhs: 0,
                    rhs: 3,
                },
                JitInstr::Return { src: 3 },
                JitInstr::Return { src: 1 },
            ],
        ))
        .expect("self-recursive function compiles");

    // Shallow recursion (depth < cap) runs fully native and is correct.
    match m.call(sum, &[5], &[0]) {
        NativeOutcome::Completed(v) => assert_eq!(v, 15),
        other => panic!("sum(5) expected Completed(15), got {other:?}"),
    }
    match m.call(sum, &[100], &[0]) {
        NativeOutcome::Completed(v) => assert_eq!(v, 5050),
        other => panic!("sum(100) expected Completed(5050), got {other:?}"),
    }

    // The embedding supplies the logical depth of this callee. A callee that
    // would already be beyond the language limit must bail even when it takes
    // its immediate base case and performs no recursive child call.
    assert!(matches!(
        m.call_with_host_ctx_at_depth(
            sum,
            &[0],
            &[0],
            0,
            &mut [],
            LogicalCallDepth {
                current: 9,
                limit: 8,
            },
        ),
        NativeOutcome::Deopt { .. }
    ));

    // Recursion deeper than the cap bails cleanly (NO host-stack overflow / crash):
    // the entry depth guard deopts, which the host re-runs on the interpreter.
    let deep = NATIVE_RECURSION_DEPTH_CAP_MAX + 50;
    match m.call(sum, &[deep], &[0]) {
        NativeOutcome::Deopt { .. } => {}
        other => panic!("sum(deep) must bail at the depth cap, got {other:?}"),
    }
}

#[cfg(feature = "recursion")]
#[test]
fn native_mutual_recursion_group_computes_and_caps_depth() {
    // is_even(n) = if n<1 {1} else is_odd(n-1); is_odd(n) = if n<1 {0} else is_even(n-1).
    // Compiled as a co-declared group via CallGroup (group_index 0=even, 1=odd).
    let is_even = f(
        1,
        4,
        vec![
            JitInstr::LoadInt { dst: 1, value: 1 },
            JitInstr::JumpIfIntCompare {
                lhs: 0,
                rhs: 1,
                op: JitCompare::Lt,
                expected: true,
                target: 5,
            },
            JitInstr::Sub {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::CallGroup {
                group_index: 1,
                dst: 3,
                args: vec![2],
            },
            JitInstr::Return { src: 3 },
            JitInstr::Return { src: 1 },
        ],
    );
    let is_odd = f(
        1,
        4,
        vec![
            JitInstr::LoadInt { dst: 1, value: 1 },
            JitInstr::JumpIfIntCompare {
                lhs: 0,
                rhs: 1,
                op: JitCompare::Lt,
                expected: true,
                target: 5,
            },
            JitInstr::Sub {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            JitInstr::CallGroup {
                group_index: 0,
                dst: 3,
                args: vec![2],
            },
            JitInstr::Return { src: 3 },
            JitInstr::LoadInt { dst: 1, value: 0 },
            JitInstr::Return { src: 1 },
        ],
    );
    let mut m = module();
    let ids = m
        .compile_recursive_group(&[is_even, is_odd])
        .expect("mutually-recursive group compiles");
    let (even, odd) = (ids[0], ids[1]);

    // Shallow mutual recursion runs fully native and is correct.
    for (n, want_even) in [(10, 1), (7, 0), (0, 1), (1, 0)] {
        match m.call(even, &[n], &[0]) {
            NativeOutcome::Completed(v) => assert_eq!(v, want_even, "is_even({n})"),
            other => panic!("is_even({n}) expected Completed, got {other:?}"),
        }
    }
    match m.call(odd, &[7], &[0]) {
        NativeOutcome::Completed(v) => assert_eq!(v, 1, "is_odd(7)"),
        other => panic!("is_odd(7) expected Completed(1), got {other:?}"),
    }

    // Recursion past the depth cap bails cleanly (no host-stack overflow).
    let deep = NATIVE_RECURSION_DEPTH_CAP_MAX + 50;
    match m.call(even, &[deep], &[0]) {
        NativeOutcome::Deopt { .. } => {}
        other => panic!("deep mutual recursion must bail at the cap, got {other:?}"),
    }
}

#[test]
fn native_scalar_call_chains_child_deopt_payload() {
    let mut m = module();
    // callee(a, b) = a + b; overflows for (MAX, 1). The child Add guard lives
    // at callee safepoint 1 with params a/b live.
    let callee = m
        .compile(&f(
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
        ))
        .unwrap();
    // caller(x, y) { c = 9; return callee(x, y) + c }. On child deopt, the
    // parent deopts at the CallNative site and preserves its own live regs plus
    // a child frame decoded with the callee's deopt map.
    let caller = m
        .compile(&f(
            2,
            5,
            vec![
                JitInstr::LoadInt { dst: 2, value: 9 },
                JitInstr::CallNative {
                    callee,
                    dst: 3,
                    args: vec![0, 1],
                },
                JitInstr::Add {
                    dst: 4,
                    lhs: 3,
                    rhs: 2,
                },
                JitInstr::Return { src: 4 },
            ],
        ))
        .unwrap();

    match m.call(caller, &[i64::MAX, 1], &[0, 0]) {
        NativeOutcome::Deopt {
            safepoint_id,
            live,
            child: Some(child),
            ..
        } => {
            assert_eq!(safepoint_id, SafepointId(1));
            assert_eq!(child.function, callee);
            assert_eq!(child.safepoint_id, SafepointId(1));
            assert_eq!(child.child, None);
            assert_eq!(
                live.iter().find(|r| r.reg == 2).map(|r| r.value),
                Some(DeoptValue::Int(9)),
                "parent payload should still capture live caller regs"
            );
            assert_eq!(
                child.live.iter().find(|r| r.reg == 0).map(|r| r.value),
                Some(DeoptValue::Int(i64::MAX))
            );
            assert_eq!(
                child.live.iter().find(|r| r.reg == 1).map(|r| r.value),
                Some(DeoptValue::Int(1))
            );
        }
        other => panic!("expected parent deopt with child frame, got {other:?}"),
    }
}

#[test]
fn native_scalar_call_chains_nested_child_deopt_payloads() {
    let mut m = module();
    let leaf = m
        .compile(&f(
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
        ))
        .unwrap();
    let middle = m
        .compile(&f(
            2,
            4,
            vec![
                JitInstr::LoadInt { dst: 2, value: 11 },
                JitInstr::CallNative {
                    callee: leaf,
                    dst: 3,
                    args: vec![0, 1],
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let top = m
        .compile(&f(
            2,
            4,
            vec![
                JitInstr::LoadInt { dst: 2, value: 22 },
                JitInstr::CallNative {
                    callee: middle,
                    dst: 3,
                    args: vec![0, 1],
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();

    match m.call(top, &[i64::MAX, 1], &[0, 0]) {
        NativeOutcome::Deopt {
            safepoint_id,
            child: Some(middle_frame),
            ..
        } => {
            assert_eq!(safepoint_id, SafepointId(1));
            assert_eq!(middle_frame.function, middle);
            assert_eq!(middle_frame.safepoint_id, SafepointId(1));
            assert_eq!(
                middle_frame
                    .live
                    .iter()
                    .find(|r| r.reg == 2)
                    .map(|r| r.value),
                Some(DeoptValue::Int(11))
            );
            let leaf_frame = middle_frame.child.as_ref().expect("leaf child frame");
            assert_eq!(leaf_frame.function, leaf);
            assert_eq!(leaf_frame.safepoint_id, SafepointId(1));
            assert_eq!(
                leaf_frame.live.iter().find(|r| r.reg == 0).map(|r| r.value),
                Some(DeoptValue::Int(i64::MAX))
            );
            assert_eq!(
                leaf_frame.live.iter().find(|r| r.reg == 1).map(|r| r.value),
                Some(DeoptValue::Int(1))
            );
            assert_eq!(leaf_frame.child, None);
        }
        other => panic!("expected nested native deopt chain, got {other:?}"),
    }
}

#[test]
fn native_call_can_pass_handle_arg_to_readonly_callee() {
    use JitValueType::{Handle, Int};
    extern "C" fn string_len(_ctx: HostCtx, handle: i64) -> i64 {
        handle * 2
    }
    let mut m = NativeModule::new(HostHelpers {
        string_len,
        ..host_helpers()
    })
    .unwrap();
    let callee = m
        .compile(&ft(
            1,
            vec![Handle, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::StringLen,
                    dst: 1,
                    args: vec![HostArg::Reg(0)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let caller = m
        .compile(&ft(
            1,
            vec![Handle, Int],
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

    assert_eq!(m.callt(caller, &[21]), Some(42));
}

#[test]
fn native_call_can_invoke_heap_writing_handle_callee() {
    use JitValueType::{Handle, Int};
    extern "C" fn list_set_int(_ctx: HostCtx, handle: i64, index: i64, value: i64) -> i64 {
        handle + index * 10 + value * 100
    }
    let mut m = NativeModule::new(HostHelpers {
        list_set_int,
        ..host_helpers()
    })
    .unwrap();
    let callee = m
        .compile(&ft(
            3,
            vec![Handle, Int, Int, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::ListSetInt,
                    dst: 3,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1), HostArg::Reg(2)],
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let caller = m
        .compile(&ft(
            3,
            vec![Handle, Int, Int, Int],
            vec![
                JitInstr::CallNative {
                    callee,
                    dst: 3,
                    args: vec![0, 1, 2],
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();

    assert_eq!(m.callt(caller, &[5, 2, 7]), Some(725));
}

#[test]
fn native_call_can_pass_flat_int_arg_to_readonly_callee() {
    use JitValueType::{FlatInt, Int};
    let mut m = module();
    let flat_callee = m
        .compile(&ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    let caller = m
        .compile(&ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::CallNative {
                    callee: flat_callee,
                    dst: 1,
                    args: vec![0],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();

    let data = [10i64, 20, 30];
    let mut flat = [FlatBufferArg::Int(&data)];
    assert_eq!(
        m.call_with_host_ctx(
            caller,
            &[data.as_ptr() as i64],
            &[data.len() as i64],
            0,
            &mut flat,
        )
        .completed(),
        Some(3)
    );
}

#[test]
fn native_call_can_pass_flat_int_arg_to_mutating_callee() {
    use JitValueType::{FlatIntMut, Int};
    let mut m = module();
    let flat_callee = m
        .compile(&ft(
            3,
            vec![FlatIntMut, Int, Int, Int],
            vec![
                JitInstr::ListSetIntDirect {
                    dst: 3,
                    base: 0,
                    index: 1,
                    value: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let caller = m
        .compile(&ft(
            3,
            vec![FlatIntMut, Int, Int, Int, Int],
            vec![
                JitInstr::CallNative {
                    callee: flat_callee,
                    dst: 3,
                    args: vec![0, 1, 2],
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 4 },
            ],
        ))
        .unwrap();

    let mut data = [10i64, 20, 30];
    let args = [data.as_mut_ptr() as i64, 1, 99];
    let lens = [data.len() as i64, 0, 0];
    let mut flat = [FlatBufferArg::IntMut(&mut data)];
    assert_eq!(
        m.call_with_host_ctx(caller, &args, &lens, 0, &mut flat,)
            .completed(),
        Some(99)
    );
    assert_eq!(data, [10, 99, 30]);
}

#[test]
fn one_mutable_flat_proof_cannot_authorize_two_abi_entries() {
    use JitValueType::{FlatIntMut, Int};
    let mut module = module();
    let function = module
        .compile(&ft(
            3,
            vec![FlatIntMut, FlatIntMut, Int],
            vec![JitInstr::Return { src: 2 }],
        ))
        .unwrap();
    let mut data = [1_i64, 2];
    let pointer = data.as_mut_ptr() as i64;
    let args = [pointer, pointer, 7];
    let lens = [2, 2, 0];
    let mut proof = [FlatBufferArg::IntMut(&mut data)];
    assert!(matches!(
        module.call_with_host_ctx(function, &args, &lens, 0, &mut proof),
        NativeOutcome::Deopt { .. }
    ));
}

#[test]
fn prepared_call_owns_abi_words_and_flat_borrow_proofs() {
    use JitValueType::{FlatIntMut, Int};
    let mut module = module();
    let function = module
        .compile(&ft(
            3,
            vec![FlatIntMut, Int, Int, Int],
            vec![
                JitInstr::ListSetIntDirect {
                    dst: 3,
                    base: 0,
                    index: 1,
                    value: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let mut data = [3_i64, 4];
    let outcome = module
        .prepare_call(function)
        .unique_int_mut(&mut data)
        .scalar(1)
        .scalar(42)
        .execute();
    assert_eq!(outcome.completed(), Some(0));
    assert_eq!(data, [3, 42]);
}

#[test]
fn scalar_machine_entry_runs_on_tiny_guarded_stacks() {
    use JitValueType::Int;
    let mut module = module();
    let function = module
        .compile(&ft(
            2,
            vec![Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    let entry = module.test_raw_entry(function).unwrap();

    for stack_bytes in [64 * 1024, 128 * 1024, 256 * 1024] {
        std::thread::Builder::new()
            .name(format!("jit-raw-stack-{stack_bytes}"))
            .stack_size(stack_bytes)
            .spawn(move || {
                let args = [20_i64, 22];
                let lens = [0_i64, 0];
                let mut result = 0_i64;
                let mut bail = 0_u8;
                let mut safepoint = 0_i64;
                let mut deopt = [0_i64; 3];
                let mut frame = JitCallFrame {
                    abi_version: JIT_CALL_ABI_VERSION,
                    frame_size: CALL_FRAME_SIZE,
                    flags: 0,
                    args: args.as_ptr(),
                    lens: lens.as_ptr(),
                    arg_count: args.len(),
                    host_ctx: 0,
                    limits: std::ptr::null(),
                    result: &mut result,
                    bail: &mut bail,
                    safepoint: &mut safepoint,
                    deopt: deopt.as_mut_ptr(),
                    native_depth: 0,
                    logical_depth: 0,
                    logical_depth_limit: usize::MAX,
                };
                // SAFETY: the entry was compiled with the one-frame ABI, the
                // module remains alive until all scoped joins complete, and every
                // pointer in the frame references live local storage.
                let status = unsafe { entry(&mut frame) };
                assert_eq!(status, JitStatus::Completed);
                assert_eq!(result, 42);
            })
            .expect("tiny-stack native worker starts")
            .join()
            .expect("scalar native entry must stay within the guarded stack");
    }
}

#[test]
fn machine_entry_rejects_incompatible_frame_prefix_before_pointer_loads() {
    use JitValueType::Int;
    let mut module = module();
    let function = module
        .compile(&ft(1, vec![Int], vec![JitInstr::Return { src: 0 }]))
        .unwrap();
    let entry = module.test_raw_entry(function).unwrap();
    let args = [41_i64];
    let lens = [0_i64];

    for (abi_version, frame_size) in [
        (JIT_CALL_ABI_VERSION + 1, CALL_FRAME_SIZE),
        (JIT_CALL_ABI_VERSION, CALL_FRAME_SIZE - 1),
    ] {
        let mut result = 99_i64;
        let mut bail = 0_u8;
        let mut safepoint = 0_i64;
        let mut deopt = [0_i64; 1];
        let mut frame = JitCallFrame {
            abi_version,
            frame_size,
            flags: 0,
            args: args.as_ptr(),
            lens: lens.as_ptr(),
            arg_count: args.len(),
            host_ctx: 0,
            limits: std::ptr::null(),
            result: &mut result,
            bail: &mut bail,
            safepoint: &mut safepoint,
            deopt: deopt.as_mut_ptr(),
            native_depth: 0,
            logical_depth: 0,
            logical_depth_limit: usize::MAX,
        };
        // SAFETY: `frame` provides the complete ABI prefix. The generated entry
        // must reject it before consulting any pointer-bearing field.
        let status = unsafe { entry(&mut frame) };
        assert_eq!(status, JitStatus::AbiMismatch);
        assert_eq!(result, 99);
        assert_eq!(bail, 0);
    }
}

#[cfg(unix)]
#[test]
fn flat_direct_bounds_checks_do_not_touch_guard_pages() {
    use JitValueType::{FlatInt, FlatIntMut, Int};

    struct Mapping {
        ptr: *mut libc::c_void,
        len: usize,
    }
    impl Drop for Mapping {
        fn drop(&mut self) {
            // SAFETY: `ptr`/`len` are the exact successful `mmap` result owned by
            // this guard and are released exactly once here.
            let result = unsafe { libc::munmap(self.ptr, self.len) };
            debug_assert_eq!(result, 0);
        }
    }

    // Put the two-element buffer at the very end of a writable page. If generated
    // code performs the access before checking `index < len`, the one-past-end
    // read/write lands in the following PROT_NONE page and the test process traps.
    // SAFETY: sysconf has no memory-safety preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    assert!(page >= 4096);
    let mapping_len = page * 3;
    // SAFETY: anonymous private mapping; the returned region is checked before use.
    let raw = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            mapping_len,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    assert_ne!(raw, libc::MAP_FAILED);
    let mapping = Mapping {
        ptr: raw,
        len: mapping_len,
    };
    // SAFETY: the middle page is contained in the live mapping.
    let protect_result = unsafe {
        libc::mprotect(
            (mapping.ptr as *mut u8).add(page).cast(),
            page,
            libc::PROT_READ | libc::PROT_WRITE,
        )
    };
    assert_eq!(protect_result, 0);
    // SAFETY: the final two i64 slots of the writable middle page are aligned,
    // initialized below, and remain live until `mapping` drops after both calls.
    let values = unsafe {
        std::slice::from_raw_parts_mut(
            (mapping.ptr as *mut u8)
                .add(page * 2 - 2 * std::mem::size_of::<i64>())
                .cast::<i64>(),
            2,
        )
    };
    values.copy_from_slice(&[11, 22]);

    let mut module = module();
    let read = module
        .compile(&ft(
            2,
            vec![FlatInt, Int, Int],
            vec![
                JitInstr::ListGetIntDirect {
                    dst: 2,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    let pointer = values.as_ptr() as i64;
    let args = [pointer, values.len() as i64];
    let lens = [values.len() as i64, 0];
    {
        let mut read_proof = [FlatBufferArg::Int(values)];
        assert!(matches!(
            module.call_with_host_ctx(read, &args, &lens, 0, &mut read_proof),
            NativeOutcome::Deopt { .. }
        ));
    }

    let write = module
        .compile(&ft(
            3,
            vec![FlatIntMut, Int, Int, Int],
            vec![
                JitInstr::ListSetIntDirect {
                    dst: 3,
                    base: 0,
                    index: 1,
                    value: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    let args = [pointer, values.len() as i64, 99];
    let lens = [values.len() as i64, 0, 0];
    let mut write_proof = [FlatBufferArg::IntMut(values)];
    assert!(matches!(
        module.call_with_host_ctx(write, &args, &lens, 0, &mut write_proof),
        NativeOutcome::Deopt { .. }
    ));
    assert_eq!(values, &[11, 22]);
}

#[test]
fn native_call_can_pass_flat_float_arg_to_readonly_callee() {
    use JitValueType::{FlatFloat, Float, Int};
    let mut m = module();
    let flat_callee = m
        .compile(&ft(
            2,
            vec![FlatFloat, Int, Float],
            vec![
                JitInstr::ListGetFloatDirect {
                    dst: 2,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    let caller = m
        .compile(&ft(
            2,
            vec![FlatFloat, Int, Float],
            vec![
                JitInstr::CallNative {
                    callee: flat_callee,
                    dst: 2,
                    args: vec![0, 1],
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();

    let data = [1.25f64, 2.5, 3.75];
    let mut flat = [FlatBufferArg::Float(&data)];
    let outcome = m.call_with_host_ctx(
        caller,
        &[data.as_ptr() as i64, 1],
        &[data.len() as i64, 0],
        0,
        &mut flat,
    );
    match outcome {
        NativeOutcome::Completed(bits) => {
            assert_eq!(f64::from_bits(bits as u64), 2.5);
        }
        other => panic!("expected flat-float native call to complete, got {other:?}"),
    }
}

#[test]
fn float_read_helpers_compile_and_bail() {
    use JitValueType::{Float, Handle, Int};
    // A module whose float helpers return a fixed value or bail by parity of
    // the slot/index, so we can exercise both the success and bail channels.
    extern "C" fn field_float(_ctx: HostCtx, _handle: i64, slot: i64) -> f64 {
        if slot == 0 {
            2.5
        } else {
            signal_bail(_ctx);
            0.0
        }
    }
    extern "C" fn list_get_float(_ctx: HostCtx, _handle: i64, index: i64) -> f64 {
        if index >= 0 {
            index as f64 + 0.5
        } else {
            signal_bail(_ctx);
            0.0
        }
    }
    let mut m = NativeModule::new(HostHelpers {
        field_float,
        list_get_float,
        ..host_helpers()
    })
    .unwrap();
    // fn(h: Handle, idx: Int) -> Float { return list[idx] }  regs 0=h,1=idx,2=res
    let id = m
        .compile(&ft(
            2,
            vec![Handle, Int, Float],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::ListGetFloat,
                    dst: 2,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    // Handle arg is opaque (helper ignores it); index 3 → 3.5.
    let got = m.callt(id, &[0, 3]).unwrap();
    assert_eq!(f64::from_bits(got as u64), 3.5);
    // Negative index → helper signals bail → None.
    assert_eq!(m.callt(id, &[0, -1]), None);

    // fn(h: Handle) -> Float { return field[1] }  → bails (slot != 0).
    let id2 = m
        .compile(&ft(
            1,
            vec![Handle, Float],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::FieldFloat,
                    dst: 1,
                    args: vec![HostArg::Reg(0), HostArg::ImmI64(1)],
                },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    assert_eq!(m.callt(id2, &[0]), None);
    let _ = Int;
}

#[cfg(feature = "speculation")]
#[test]
fn guard_closure_id_passes_or_bails() {
    use JitValueType::{Handle, Int};
    // `closure_id` is the identity on the handle arg, so the handle value IS the
    // observed function id — letting the test drive the guard directly.
    extern "C" fn closure_id(_ctx: HostCtx, handle: i64) -> i64 {
        handle
    }
    let mut m = NativeModule::new(HostHelpers {
        closure_id,
        ..host_helpers()
    })
    .unwrap();
    // fn(h: Handle, x: Int) -> Int { guard id(h) == 7; return x + 100 }
    // regs 0=h, 1=x, 2=hundred, 3=res
    let id = m
        .compile(&ft(
            2,
            vec![Handle, Int, Int, Int],
            vec![
                JitInstr::GuardClosureId {
                    base: 0,
                    expected: 7,
                },
                JitInstr::LoadInt { dst: 2, value: 100 },
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .unwrap();
    // Matching callee (handle == expected 7): native completes.
    assert_eq!(m.callt(id, &[7, 5]), Some(105));
    // Mismatched callee (handle != 7): guard bails to the interpreter (None).
    assert_eq!(m.callt(id, &[8, 5]), None);
    assert_eq!(m.callt(id, &[0, 5]), None);
}

#[test]
fn closure_id_dispatch_selects_arm_or_bails() {
    use JitValueType::{Bool, Handle, Int};
    // `closure_id` is the identity on the handle arg, so the handle value IS the
    // observed function id — the test drives the polymorphic dispatch directly,
    // mirroring the producer's lowering (read id once via ClosureId, then
    // LoadInt + Equal + JumpIfBool per arm, with a no-match Bail).
    extern "C" fn closure_id(_ctx: HostCtx, handle: i64) -> i64 {
        handle
    }
    let mut m = NativeModule::new(HostHelpers {
        closure_id,
        ..host_helpers()
    })
    .unwrap();
    // Polymorphic inline cache over two callees {3, 5}:
    //   0: id  = closure_id(h)
    //   1: key = 3
    //   2: eq  = (id == key)
    //   3: if eq -> arm3 (8)
    //   4: key = 5
    //   5: eq  = (id == key)
    //   6: if eq -> arm5 (11)
    //   7: Bail                    ; no match
    //   8: c30 = 30  9: res = x+30  10: return res   (arm3)
    //  11: c50 = 50 12: res = x+50  13: return res   (arm5)
    // regs: 0=h(Handle) 1=x 2=id 3=key 4=eq 5=c30 6=res3 7=c50 8=res5
    let id = m
        .compile(&ft(
            2,
            vec![Handle, Int, Int, Int, Bool, Int, Int, Int, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::ClosureId,
                    dst: 2,
                    args: vec![HostArg::Reg(0)],
                }, // 0
                JitInstr::LoadInt { dst: 3, value: 3 }, // 1
                JitInstr::Equal {
                    dst: 4,
                    lhs: 2,
                    rhs: 3,
                }, // 2
                JitInstr::JumpIfBool {
                    cond: 4,
                    expected: true,
                    target: 8,
                }, // 3
                JitInstr::LoadInt { dst: 3, value: 5 }, // 4
                JitInstr::Equal {
                    dst: 4,
                    lhs: 2,
                    rhs: 3,
                }, // 5
                JitInstr::JumpIfBool {
                    cond: 4,
                    expected: true,
                    target: 11,
                }, // 6
                JitInstr::Bail,                         // 7 (no match)
                JitInstr::LoadInt { dst: 5, value: 30 }, // 8 arm3
                JitInstr::Add {
                    dst: 6,
                    lhs: 1,
                    rhs: 5,
                }, // 9
                JitInstr::Return { src: 6 },            // 10
                JitInstr::LoadInt { dst: 7, value: 50 }, // 11 arm5
                JitInstr::Add {
                    dst: 8,
                    lhs: 1,
                    rhs: 7,
                }, // 12
                JitInstr::Return { src: 8 },            // 13
            ],
        ))
        .unwrap();
    // Arm 3 selected (h == 3): x + 30.
    assert_eq!(m.callt(id, &[3, 5]), Some(35));
    // Arm 5 selected (h == 5): x + 50.
    assert_eq!(m.callt(id, &[5, 5]), Some(55));
    // No arm matches (h == 9): the cache bails to the interpreter (None).
    assert_eq!(m.callt(id, &[9, 5]), None);
    assert_eq!(m.callt(id, &[0, 5]), None);
}

#[test]
fn direct_flat_reads_index_in_register() {
    use JitValueType::{FlatFloat, FlatInt, Float, Int};
    let mut m = module();

    // fn(a: FlatInt, i: Int) -> Int { return a[i] }  regs 0=a,1=i,2=res
    let id_int = m
        .compile(&ft(
            2,
            vec![FlatInt, Int, Int],
            vec![
                JitInstr::ListGetIntDirect {
                    dst: 2,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    let ints: Vec<i64> = vec![10, 20, 30];
    let ints_ptr = ints.as_ptr() as i64;
    let ilen = ints.len() as i64;
    // In-bounds reads index directly out of the flat buffer.
    assert_eq!(
        m.call_with_host_ctx(
            id_int,
            &[ints_ptr, 0],
            &[ilen, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        )
        .completed(),
        Some(10)
    );
    assert_eq!(
        m.call_with_host_ctx(
            id_int,
            &[ints_ptr, 2],
            &[ilen, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        )
        .completed(),
        Some(30)
    );
    // OOB (>= len and < 0) → fallback (None), like the helper's bail.
    assert_eq!(
        m.call_with_host_ctx(
            id_int,
            &[ints_ptr, 3],
            &[ilen, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        )
        .completed(),
        None
    );
    assert_eq!(
        m.call_with_host_ctx(
            id_int,
            &[ints_ptr, -1],
            &[ilen, 0],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        )
        .completed(),
        None
    );

    // fn(a: FlatFloat, i: Int) -> Float { return a[i] }
    let id_f = m
        .compile(&ft(
            2,
            vec![FlatFloat, Int, Float],
            vec![
                JitInstr::ListGetFloatDirect {
                    dst: 2,
                    base: 0,
                    index: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        ))
        .unwrap();
    let floats: Vec<f64> = vec![1.5, 2.5, 3.5];
    let fptr = floats.as_ptr() as i64;
    let flen = floats.len() as i64;
    let read = |i: i64| {
        m.call_with_host_ctx(
            id_f,
            &[fptr, i],
            &[flen, 0],
            0,
            &mut [FlatBufferArg::Float(&floats)],
        )
        .completed()
        .map(|b| f64::from_bits(b as u64))
    };
    assert_eq!(read(1), Some(2.5));
    assert_eq!(read(0), Some(1.5));
    assert_eq!(read(3), None);

    // fn(a: FlatInt) -> Int { return len(a) }  via ListLenDirect
    let id_len = m
        .compile(&ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        ))
        .unwrap();
    assert_eq!(
        m.call_with_host_ctx(
            id_len,
            &[ints_ptr],
            &[ilen],
            0,
            &mut [FlatBufferArg::Int(&ints)]
        )
        .completed(),
        Some(3)
    );
}
use super::*;
