    #[cfg(feature = "native-jit")]
    #[test]
    fn tier0_jit_pure_leaf_call_still_obeys_logical_depth_limit() {
        let callee = native_test_function(
            "plus_one",
            1,
            3,
            vec![
                RegInstr::LoadInt { dst: 1, value: 1 },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::Return { src: 2 },
            ],
        );
        let caller = native_test_function(
            "caller",
            1,
            2,
            vec![
                RegInstr::CallKnown {
                    dst: 1,
                    function: 0,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let unit = Rc::new(native_test_unit(vec![callee, caller]));
        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.set_limits(VmLimits {
            max_depth: 0,
            ..VmLimits::default()
        });
        let caller = Rc::clone(&unit.functions[1]);
        vm.prepare_frame(0, caller.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(41));

        let error = vm
            .run_jit(&unit, caller.as_ref(), 0)
            .expect_err("an unframed pure leaf call is still a logical call");

        assert!(matches!(
            error,
            EvalError::Runtime(message)
                if message.contains("recursion depth limit exceeded (0 frames)")
        ));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_prefilter_keeps_scalar_replaceable_aggregates_for_translation() {
        let variant_layout = native_test_layout("Boxed", &["value"]);
        let struct_layout = native_test_layout("Pair", &["a", "b"]);
        let functions = vec![
            native_test_function(
                "variant_hot",
                1,
                5,
                vec![
                    RegInstr::MakeVariant {
                        dst: 1,
                        layout: Rc::clone(&variant_layout),
                        fields: vec![("value".to_string(), 0)],
                    },
                    RegInstr::MatchVariant {
                        src: 1,
                        expected: "Boxed".to_string(),
                        match_ip: 2,
                        else_ip: 4,
                    },
                    RegInstr::UnwrapVariantValue {
                        dst: 2,
                        src: 1,
                        expected: "Boxed".to_string(),
                    },
                    RegInstr::Return { src: 2 },
                    RegInstr::Return { src: 0 },
                ],
            ),
            native_test_function(
                "struct_hot",
                1,
                5,
                vec![
                    RegInstr::LoadInt { dst: 1, value: 3 },
                    RegInstr::MakeStruct {
                        dst: 2,
                        layout: Rc::clone(&struct_layout),
                        fields: vec![("a".to_string(), 0), ("b".to_string(), 1)],
                    },
                    RegInstr::GetFieldSlot {
                        dst: 3,
                        base: 2,
                        slot: 0,
                    },
                    RegInstr::Return { src: 3 },
                ],
            ),
        ];

        mark_predictably_native_ineligible(&functions);

        assert_eq!(
            functions[0].native_status.get(),
            0,
            "scalar-replaceable variants must reach the real native translator",
        );
        assert_eq!(
            functions[1].native_status.get(),
            0,
            "scalar-replaceable structs must reach the real native translator",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_prefilter_keeps_sinkable_closures_for_translation() {
        let callee = native_test_function(
            "mapper",
            1,
            3,
            vec![
                RegInstr::LoadInt { dst: 1, value: 1 },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::Return { src: 2 },
            ],
        );
        let caller = native_test_function(
            "closure_hot",
            1,
            3,
            vec![
                RegInstr::MakeClosure {
                    dst: 1,
                    function: 0,
                    captures: Vec::new(),
                },
                RegInstr::CallClosure {
                    dst: 2,
                    closure: 1,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 2 },
            ],
        );
        let functions = vec![callee, caller];

        mark_predictably_native_ineligible(&functions);

        assert_eq!(
            functions[1].native_status.get(),
            0,
            "sinkable local closures must reach the real native translator",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_prefilter_marks_structurally_unsupported_functions_not_eligible() {
        let functions = vec![native_test_function(
            "build_list",
            0,
            2,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::MakeList {
                    dst: 1,
                    items: vec![0],
                },
                RegInstr::Return { src: 0 },
            ],
        )];

        mark_predictably_native_ineligible(&functions);

        assert_eq!(
            functions[0].native_status.get(),
            NATIVE_STATUS_NOT_ELIGIBLE,
            "reachable heap construction cannot translate to the native read-only subset",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_prefilter_keeps_inlinable_callers_for_translation() {
        let callee = native_test_function(
            "clampish",
            3,
            3,
            vec![
                RegInstr::JumpIfIntCompare {
                    lhs: 0,
                    rhs: 1,
                    op: RegIntCompare::Less,
                    expected: true,
                    target: 3,
                },
                RegInstr::JumpIfIntCompare {
                    lhs: 0,
                    rhs: 2,
                    op: RegIntCompare::Greater,
                    expected: true,
                    target: 4,
                },
                RegInstr::Return { src: 0 },
                RegInstr::Return { src: 1 },
                RegInstr::Return { src: 2 },
            ],
        );
        let caller = native_test_function(
            "caller",
            1,
            4,
            vec![
                RegInstr::LoadInt { dst: 1, value: 0 },
                RegInstr::LoadInt { dst: 2, value: 10 },
                RegInstr::CallKnown {
                    dst: 3,
                    function: 0,
                    args: vec![0, 1, 2],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 3 },
            ],
        );
        let functions = vec![callee, caller];

        mark_predictably_native_ineligible(&functions);

        assert_eq!(
            functions[0].native_status.get(),
            0,
            "branchy scalar callees are still native-inlinable",
        );
        assert_eq!(
            functions[1].native_status.get(),
            0,
            "callers that only leave the subset through an inlinable CallKnown must still reach the translator",
        );
    }

    /// Execution spec §6.2 (Model A): native (Cranelift) code polls neither the
    /// step budget nor the cancel flag, so dispatching to it while either is armed
    /// would let a hot loop bypass the limit. `try_native` MUST refuse to dispatch
    /// in that case and fall back to the ticking tier-0/interpreter path. The guard
    /// is the very first thing in `try_native`, so it returns before any tiering or
    /// stat bookkeeping — `considered`/`tier_deferred` stay at 0, proving the
    /// function never even entered the native machinery.
    #[cfg(feature = "native-jit")]
    #[test]
    fn try_native_refuses_dispatch_while_step_budget_armed() {
        let mut vm = empty_vm();
        // threshold 0 => without the gate this function would tier up on the first
        // call; collect_stats so we can observe the native machinery never runs.
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.limits = VmLimits {
            step_budget: Some(1_000),
            ..VmLimits::default()
        };
        let func = RegFunction::placeholder("hot".to_string());
        assert!(
            matches!(vm.try_native(&func, 0), NativeAttempt::Fallback),
            "an armed step_budget must make native dispatch refuse",
        );
        let stats = &vm.native.as_ref().unwrap().stats;
        assert_eq!(stats.considered, 0, "gate must return before tiering");
        assert_eq!(stats.tier_deferred, 0, "gate must return before tiering");
        assert_eq!(stats.native_calls, 0);
    }

    /// Same gate for the ambient `cancel` hook: a watchdog flag that can fire mid-
    /// run also makes native ineligible (it cannot poll the flag).
    #[cfg(feature = "native-jit")]
    #[test]
    fn try_native_refuses_dispatch_while_cancel_armed() {
        let mut vm = empty_vm();
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.limits = VmLimits {
            cancel: Some(rsscript_operation::CancellationToken::new()),
            ..VmLimits::default()
        };
        let func = RegFunction::placeholder("hot".to_string());
        assert!(
            matches!(vm.try_native(&func, 0), NativeAttempt::Fallback),
            "a present cancel hook must make native dispatch refuse",
        );
        assert_eq!(vm.native.as_ref().unwrap().stats.considered, 0);
    }

    /// Builds a structurally native-eligible unary function `f(x) = x + 1`. The
    /// native type-predictor sees `x` combined with an `Int` via `AddInt`, so it
    /// infers the parameter as `Int` and the function compiles. Calling it with a
    /// non-`Int` (heap) argument therefore bails at the arg-marshal site on *every*
    /// call. Used to exercise the predict-and-skip give-up path.
    #[cfg(feature = "native-jit")]
    fn always_arg_mismatch_func() -> RegFunction {
        // reg 0 = param `x`; reg 1 = constant 1.
        let code = vec![
            RegInstr::LoadInt { dst: 1, value: 1 },
            RegInstr::AddInt {
                dst: 0,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::Return { src: 0 },
        ];
        RegFunction {
            name: "f".to_string(),
            params: 1,
            captures: 0,
            regs: 2,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    /// Predict-and-skip bail: a function that PASSES the
    /// structural predictor (it compiles) but bails at runtime on *every* call must
    /// not re-compile/marshal/bail forever. After `NATIVE_BAIL_GIVEUP_THRESHOLD`
    /// consecutive bails the native tier gives up on that shape and negative-caches
    /// its version, so costly marshalling failures plateau at the threshold even
    /// though hot calls continue consulting the shape cache. Here the bail is an
    /// arg-type mismatch (Int param, heap argument).
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_gives_up_after_consecutive_bails() {
        let mut vm = empty_vm();
        // threshold 0 => compile/attempt on the very first call; collect_stats so we
        // can observe the attempt count.
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = always_arg_mismatch_func();

        // Place a heap (List) value in the function's single parameter register, so
        // marshalling the `Int`-typed param fails on every native attempt.
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::new(RefCell::new(TypedVec::new()))));

        const CALLS: usize = 50;
        for _ in 1..=CALLS {
            // Native is never chosen (it bails), so the result is always `None`
            // (fall back to the interpreter).
            assert!(
                matches!(vm.try_native(&func, 0), NativeAttempt::Fallback),
                "always-mismatching native call must bail to the interpreter",
            );
        }

        assert_ne!(
            func.native_status.get(),
            NATIVE_STATUS_NOT_ELIGIBLE,
            "runtime mismatch must not globally demote a structurally eligible function",
        );

        let stats = &vm.native.as_ref().unwrap().stats;
        // The shape lookup is still considered on every hot call, but expensive
        // marshalling failures plateau once the version is negative-cached.
        assert_eq!(
            stats.considered, CALLS as u64,
            "hot calls still consult the bounded shape cache",
        );
        assert_eq!(
            stats.arg_mismatch, NATIVE_BAIL_GIVEUP_THRESHOLD as u64,
            "bail count must plateau at the give-up threshold",
        );
        let native = vm.native.as_ref().unwrap();
        let version_key = NativeVersionKey {
            function: &func as *const RegFunction as usize,
            shape: ShapeKey::from_values([vm.reg(0)]),
        };
        assert_eq!(
            native.cache.get(&version_key),
            Some(&None),
            "failing shape must be negative-cached on give-up",
        );
    }

    /// A native-eligible function `f(x) = { let a = x + 1; return a * a }`. The
    /// `MulInt` overflows i64 for a large `x`, so native bails inside that guard —
    /// a REAL, mapped safepoint *after* `a` (reg 1) has been computed. `a` is a
    /// non-param live register, so the J0.2 precise path must restore it and
    /// resume AT the multiply.
    #[cfg(feature = "native-jit")]
    fn add_then_square_func() -> RegFunction {
        // reg 0 = param `x`; reg 1 = const 1 then `a = x + 1`; reg 2 = `a * a`.
        let code = vec![
            RegInstr::LoadInt { dst: 1, value: 1 },
            RegInstr::AddInt {
                dst: 1,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::MulInt {
                dst: 2,
                lhs: 1,
                rhs: 1,
            },
            RegInstr::Return { src: 2 },
        ];
        RegFunction {
            name: "f".to_string(),
            params: 1,
            captures: 0,
            regs: 3,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    /// J0.2 white-box: with `precise_deopt` on, a real native guard bail must
    /// return [`NativeAttempt::Resumed`], set the active frame's `ip` to the
    /// bailing instruction's `resume_ip`, and reconstruct the captured non-param
    /// live registers into the window (params are left untouched). This is the
    /// mechanical proof that resume-at-safepoint engages (a black-box output test
    /// can't distinguish it from re-run-from-top, since the subset is
    /// side-effect-free and both produce the same value by design).
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_resumes_at_safepoint_and_restores_live_regs() {
        let mut vm = empty_vm();
        // threshold 0 => compile/attempt on the first call; precise_deopt on.
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );
        let func = Rc::new(add_then_square_func());

        // Window at base 0; push an active frame so the precise path can set its
        // `ip`. A large `x` makes `a * a` overflow i64 → real guard bail.
        let big: i64 = 4_000_000_000;
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Resumed),
            "a real guard bail under precise_deopt must resume at the safepoint",
        );

        // The frame ip must now be the bailing instruction's resume_ip: the
        // `MulInt` at code index 2.
        let frame_ip = vm.frames.last().expect("frame").ip;
        assert_eq!(
            frame_ip, 2,
            "frame ip must be set to the bailing instruction's resume_ip (the MulInt)",
        );

        // The non-param live register `a` (reg 1) must be reconstructed to the
        // native-computed `x + 1`; the param (reg 0) is left as the window held it.
        assert_eq!(
            *vm.reg(1),
            VmValue::Int(big + 1),
            "non-param live register must be restored from the captured deopt value",
        );
        assert_eq!(
            *vm.reg(0),
            VmValue::Int(big),
            "param register must be left untouched by precise reconstruction",
        );
    }

    /// B2 (FIXED by J0.1 heap-aware deopt state maps): the reg VM binds params as
    /// locals, so `n = n + 1` rewrites the param register. A native scalar function
    /// that REASSIGNS a param still live at a safepoint must, on precise deopt, restore
    /// the param to its native-computed value. The deopt state map distinguishes
    /// reconstructible scalars (`Int`/`Float`) from heap refs (`Handle`/`FlatInt`/
    /// `FlatFloat`): `decode_deopt_live` drops the latter (the frame already holds
    /// their `VmValue`), so `restore_native_deopt_live_regs` can restore ALL scalar
    /// regs — params included — without the old `< n_params` skip that lost reassigned
    /// scalar params. (Skipping only `Handle` and not `FlatInt`/`FlatFloat` corrupts
    /// flat-buffer params — see `native_heap_reads`/`tv2_direct_flat_reads`.)
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_restores_reassigned_scalar_param() {
        let big: i64 = 4_000_000_000;
        // reg 0 = param x (runtime `big`); reg 0 = x + 1 (REASSIGN, live past the
        // safepoint); reg 2 = reg0 * reg0 -> runtime overflow guard bail; reg 2 =
        // reg0 + reg2 keeps reg 0 live at the MulInt safepoint.
        let func = Rc::new(native_test_function(
            "reassign",
            1,
            3,
            vec![
                RegInstr::LoadInt { dst: 1, value: 1 },
                RegInstr::AddInt {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::MulInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 0,
                },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 2,
                },
                RegInstr::Return { src: 2 },
            ],
        ));
        let mut vm = empty_vm();
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Resumed),
            "the overflow guard must precisely resume",
        );
        assert_eq!(
            *vm.reg(0),
            VmValue::Int(big + 1),
            "a reassigned scalar param must be restored to its native-computed value \
             (big + 1), not the stale call-time `big`",
        );
    }

    /// Staged native-call ABI: when a scalar native callee deopts, RSScript must
    /// reconstruct the logical caller+callee interpreter frames instead of
    /// resuming at the caller's `CallKnown` site or falling back from the top.
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_with_child_native_frame_resumes_child_safepoint() {
        let child = add_then_square_func();
        let caller = native_test_function(
            "caller",
            1,
            2,
            vec![
                RegInstr::CallKnown {
                    dst: 1,
                    function: 0,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let unit = Rc::new(native_test_unit(vec![child, caller]));
        let caller = Rc::clone(&unit.functions[1]);

        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );

        let big: i64 = 4_000_000_000;
        vm.prepare_frame(0, caller.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&caller),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&caller, 0);
        assert!(
            matches!(outcome, NativeAttempt::Resumed),
            "nested native callee deopt must reconstruct the interpreter frame chain",
        );
        assert_eq!(
            vm.frames.len(),
            2,
            "resume should leave caller suspended below the deopted child frame",
        );
        assert_eq!(
            vm.frames[0].ip, 1,
            "caller frame should resume after its CallKnown once the child returns",
        );
        assert_eq!(
            vm.frames[1].ip, 2,
            "child frame should resume at the overflowing MulInt safepoint",
        );
        assert_eq!(
            *vm.reg(caller.regs + 1),
            VmValue::Int(big + 1),
            "child non-param live register should be restored from the nested payload",
        );

        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.native_bails, 1);
        assert_eq!(
            stats.native_child_bails, 1,
            "nested callee deopt should be visible in native telemetry",
        );
        assert_eq!(
            stats.native_child_resumes, 1,
            "nested callee deopt should be counted as a frame-chain resume",
        );
        assert!(
            stats.native_call_edges >= 1,
            "caller should compile with a native-to-native edge, stats={stats:?}",
        );
    }

    /// Staged native-call ABI: a nested native call chain must preserve the full
    /// deopt frame chain, not just the first child frame. This pins the RSScript
    /// embedding of vm-jit's nested `DeoptFrame` payloads.
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_with_nested_child_native_frames_resumes_leaf_safepoint() {
        let leaf = add_then_square_func();
        let middle = native_test_function(
            "middle",
            1,
            3,
            vec![
                RegInstr::CallKnown {
                    dst: 2,
                    function: 0,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 2 },
            ],
        );
        let top = native_test_function(
            "top",
            1,
            2,
            vec![
                RegInstr::CallKnown {
                    dst: 1,
                    function: 1,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let unit = Rc::new(native_test_unit(vec![leaf, middle, top]));
        let top = Rc::clone(&unit.functions[2]);

        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );

        let big: i64 = 4_000_000_000;
        vm.prepare_frame(0, top.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&top),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&top, 0);
        assert!(
            matches!(outcome, NativeAttempt::Resumed),
            "nested native leaf deopt must reconstruct all interpreter frames",
        );
        assert_eq!(
            vm.frames.len(),
            3,
            "resume should leave top, middle, and deopted leaf frames",
        );
        assert_eq!(
            vm.frames[0].ip, 1,
            "top frame should resume after its CallKnown once middle returns",
        );
        assert_eq!(
            vm.frames[1].ip, 1,
            "middle frame should resume after its CallKnown once leaf returns",
        );
        assert_eq!(
            vm.frames[2].ip, 2,
            "leaf frame should resume at the overflowing MulInt safepoint",
        );

        let leaf_base = top.regs + unit.functions[1].regs;
        assert_eq!(
            *vm.reg(leaf_base + 1),
            VmValue::Int(big + 1),
            "leaf non-param live register should be restored from the nested payload",
        );

        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.native_bails, 1);
        assert_eq!(
            stats.native_child_bails, 1,
            "nested leaf deopt should be visible in native telemetry",
        );
        assert_eq!(
            stats.native_child_resumes, 1,
            "nested leaf deopt should be counted as one frame-chain resume",
        );
        assert!(
            stats.native_call_edges >= 2 && stats.native_call_depth_max >= 2,
            "top should compile as a nested native-call chain, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_deopt_every_env_parser_is_fail_closed() {
        assert!(!jit_native_deopt_every_from_env_value(None));
        assert!(!jit_native_deopt_every_from_env_value(Some("")));
        assert!(!jit_native_deopt_every_from_env_value(Some("0")));
        assert!(!jit_native_deopt_every_from_env_value(Some("false")));
        assert!(!jit_native_deopt_every_from_env_value(Some("FALSE")));
        assert!(jit_native_deopt_every_from_env_value(Some("1")));
        assert!(jit_native_deopt_every_from_env_value(Some("true")));
        assert!(jit_native_deopt_every_from_env_value(Some("yes")));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_native_safepoints_match_interpreter_output() {
        let source = r#"
fn calc(x: Int) -> Int {
    let a = x + 1
    let b = a * 3
    let c = b / 2
    return c + a
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: calc(x: read 7)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        for safepoint in 1..=4 {
            let actual = reg_vm_eval_source_main_native_force_safepoint(
                "forced-safepoint.rss",
                source,
                std::iter::empty::<&str>(),
                safepoint,
            )
            .unwrap_or_else(|error| panic!("forced safepoint {safepoint} failed: {error:?}"));
            assert_eq!(
                actual.stdout, expected.stdout,
                "forced safepoint {safepoint} must preserve observable output",
            );
            assert_eq!(
                actual.value, expected.value,
                "forced safepoint {safepoint} must preserve main value",
            );
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_all_native_safepoints_match_interpreter_output() {
        let source = r#"
fn calc(x: Int) -> Int {
    let a = x + 1
    let b = a * 3
    let c = b / 2
    return c + a
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: calc(x: read 7)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-all-safepoints.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        let actual = reg_vm_eval_source_main_native_force_all_safepoints(
            "forced-all-safepoints.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("native deopt-every-safepoint run");
        assert_eq!(
            actual.stdout, expected.stdout,
            "deopt-every-safepoint mode must preserve observable stdout",
        );
        assert_eq!(
            actual.value, expected.value,
            "deopt-every-safepoint mode must preserve main value",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_child_native_safepoint_resumes_and_returns_through_caller() {
        let source = r#"
fn child(x: Int) -> Int {
    let a = x + 1
    return a * a
}

fn caller(x: Int) -> Int {
    return child(x: read x) + 1
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: caller(x: read 4)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-child-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        let actual = reg_vm_eval_source_main_native_force_safepoint(
            "forced-child-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
            2,
        )
        .expect("native precise child safepoint");
        assert_eq!(actual.stdout, "26\n");
        assert_eq!(
            actual.stdout, expected.stdout,
            "child-frame precise deopt must resume, return into caller, and preserve stdout",
        );
        assert_eq!(
            actual.value, expected.value,
            "child-frame precise deopt must preserve main value",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_all_native_safepoints_exercises_child_native_deopt() {
        let source = r#"
fn child(x: Int) -> Int {
    let a = x + 1
    return a * a
}

fn caller(x: Int) -> Int {
    return child(x: read x)
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: caller(x: read 4)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-all-child-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        let executable =
            reg_vm_compile_source("forced-all-child-safepoint.rss", source).expect("compile");
        let (actual, stats) = executable
            .eval_main_with_args_native_inner(
                std::iter::empty::<&str>(),
                0,
                false,
                true,
                true,
                false,
                None,
                true,
            )
            .expect("native deopt-every child call");

        assert_eq!(actual.stdout, "25\n");
        assert_eq!(
            actual.stdout, expected.stdout,
            "deopt-every-safepoint mode must preserve stdout through a child native call",
        );
        assert_eq!(
            actual.value, expected.value,
            "deopt-every-safepoint mode must preserve main value through a child native call",
        );
        assert!(
            stats.native_call_edges >= 1 && stats.native_child_bails >= 1,
            "test must exercise a native-to-native child deopt, stats={stats:?}",
        );
        assert!(
            stats.native_child_resumes >= 1,
            "child deopt should resume through the native frame chain, stats={stats:?}",
        );
    }

    /// Flag-off default: the SAME real guard bail must take the safe fallback
    /// (re-run-from-top), leaving the frame `ip` at 0 — byte-identical to today.
    #[cfg(feature = "native-jit")]
    #[test]
    fn deopt_without_precise_flag_falls_back_from_top() {
        let mut vm = empty_vm();
        // precise_deopt OFF (the last arg).
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, false, false, false)
                .expect("native module"),
        );
        let func = Rc::new(add_then_square_func());

        let big: i64 = 4_000_000_000;
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Fallback),
            "with the flag off, a guard bail must fall back to re-run-from-top",
        );
        assert_eq!(
            vm.frames.last().expect("frame").ip,
            0,
            "fallback must leave the frame ip at 0 (re-run from the top)",
        );
    }

    /// A native pass-through `fn id(xs) { let _ = List.len(xs); return xs }`: the
    /// `ListLen` types `xs` (reg 0) as a `Handle` parameter, and the function returns
    /// that handle unchanged. This is the original heap-result pass-through
    /// producer: NO allocation and NO mutation (native just returns a value it was
    /// given). `dst` reg 1 holds the (discarded) length.
    #[cfg(feature = "native-jit")]
    fn list_passthrough_func() -> RegFunction {
        let code = vec![
            RegInstr::ListLen { dst: 1, list: 0 },
            RegInstr::Return { src: 0 },
        ];
        RegFunction {
            name: "id".to_string(),
            params: 1,
            captures: 0,
            regs: 2,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    #[cfg(feature = "native-jit")]
    fn string_from_int_return_func() -> RegFunction {
        let code = vec![
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringFromInt,
                args: vec![0],
                dst: 1,
            },
            RegInstr::Return { src: 1 },
        ];
        RegFunction {
            name: "to_string".to_string(),
            params: 1,
            captures: 0,
            regs: 2,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    #[cfg(feature = "native-jit")]
    fn string_from_int_len_func() -> RegFunction {
        let code = vec![
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringFromInt,
                args: vec![0],
                dst: 1,
            },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args: vec![1],
                dst: 2,
            },
            RegInstr::Return { src: 2 },
        ];
        RegFunction {
            name: "to_string_len".to_string(),
            params: 1,
            captures: 0,
            regs: 3,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    #[cfg(feature = "native-jit")]
    fn string_concat_len_func() -> RegFunction {
        let code = vec![
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringFromInt,
                args: vec![1],
                dst: 2,
            },
            RegInstr::StringConcat {
                dst: 3,
                left: 0,
                right: 2,
            },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args: vec![3],
                dst: 4,
            },
            RegInstr::Return { src: 4 },
        ];
        RegFunction {
            name: "concat_len".to_string(),
            params: 2,
            captures: 0,
            regs: 5,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    /// Heap-result return ABI pass-through: a native function that returns a heap
    /// PARAMETER unchanged produces the interpreter-identical heap value, while the
    /// output table remains a clean per-call scratch area. Also asserts both tables
    /// are cleared on exit — the §7.2 invariant.
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_result_passthrough_round_trips() {
        let mut vm = empty_vm();
        // threshold 0 => compile/attempt on the first call.
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(list_passthrough_func());

        // Distinct list value so identity is observable through the round-trip.
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(11),
            VmValue::Int(22),
            VmValue::Int(33),
        ])));
        let arg = VmValue::List(Rc::clone(&list));

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, arg.clone());
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        match outcome {
            NativeAttempt::Completed(value) => {
                // The native result must equal the interpreter's: the same list (same
                // backing `Rc`, same contents).
                match value {
                    VmValue::List(got) => {
                        assert!(
                            Rc::ptr_eq(&got, &list),
                            "pass-through must return the SAME backing list",
                        );
                        assert_eq!(
                            got.borrow().len(),
                            3,
                            "round-tripped list must have its original contents",
                        );
                    }
                    other => panic!("expected a List result, got {other:?}"),
                }
            }
            NativeAttempt::Resumed => {
                panic!("pass-through must complete with a heap result, got Resumed")
            }
            NativeAttempt::Fallback => {
                panic!("pass-through must complete with a heap result, got Fallback")
            }
        }

        // §7.2 invariant: the output table is cleared on EVERY exit, so nothing is
        // retained past the call (and the input table too).
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "output table must be cleared on exit"
            )
        });
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_args.is_empty(),
                "input table must be cleared on exit"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_commits_clean_handle_result() {
        let mut tx = JitHeapTransactionGuard::begin();

        let handle = rss_jit_string_from_int(JitCallCtx::active_token(), 42);
        JIT_CALL_CTX.with(|ctx| {
            assert_eq!(
                ctx.borrow().heap_results.len(),
                1,
                "helper allocation should be staged before commit"
            )
        });

        match tx.commit_handle_with_writebacks(handle, &[]) {
            Some((VmValue::String(value), writebacks)) => {
                assert_eq!(&*value, "42");
                assert!(writebacks.is_empty());
            }
            Some((other, _)) => panic!("expected committed String result, got {other:?}"),
            None => panic!("expected staged handle to commit"),
        }
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "commit must clear staged heap results"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_abort_discards_staged_helper_result() {
        {
            let mut tx = JitHeapTransactionGuard::begin();

            let handle = rss_jit_string_from_int(JitCallCtx::active_token(), 7);
            assert!(
                handle < 0,
                "heap-producing helper should return an output-table handle"
            );
            JIT_CALL_CTX.with(|ctx| {
                assert_eq!(
                    ctx.borrow().heap_results.len(),
                    1,
                    "helper allocation should be staged before abort"
                )
            });

            tx.abort();
        }

        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "abort must discard staged helper allocations"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_helpers_fail_closed_outside_active_call_context() {
        {
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                vec![VmValue::Int(1), VmValue::Int(2)],
            )))));
            assert_eq!(rss_jit_list_len(JitCallCtx::active_token(), 0), 2);
        }

        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            assert_eq!(
                ctx.active_depth, 0,
                "test should be outside a native context"
            );
            assert!(
                ctx.heap_args.is_empty(),
                "dropping the call guard must clear heap inputs",
            );
        });

        assert_eq!(
            rss_jit_list_len(JitCallCtx::active_token(), 0),
            0,
            "a helper read outside an active native context must fail closed",
        );
        assert_eq!(
            rss_jit_string_from_int(JitCallCtx::active_token(), 7),
            0,
            "a helper allocation outside an active native context must not stage output",
        );
        assert_eq!(
            rss_jit_list_set_int(JitCallCtx::active_token(), 0, 0, 99),
            0,
            "a mutating helper outside an active native context must fail closed",
        );
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "inactive helper allocation must not leave staged heap output",
            );
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_helpers_reject_wrong_active_context_token() {
        let _heap_guard = JitCallCtxGuard::enter();
        let list = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ])));
        JitCallCtx::push_heap_arg(VmValue::List(Rc::clone(&list)));

        let token = JitCallCtx::active_token();
        assert_ne!(token, 0);
        assert_eq!(rss_jit_list_len(token, 0), 2);

        let wrong_token = token.wrapping_add(1).max(1);
        assert_ne!(wrong_token, token);
        assert_eq!(
            rss_jit_list_len(wrong_token, 0),
            0,
            "a helper read with the wrong active token must fail closed",
        );
        assert_eq!(
            rss_jit_list_set_int(wrong_token, 0, 0, 99),
            0,
            "a mutating helper with the wrong active token must fail closed",
        );
        assert_eq!(
            list.borrow().get(0),
            Some(VmValue::Int(1)),
            "wrong-token mutation must not reach the list",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_abort_restores_list_set_int_write() {
        let mut values = Vec::with_capacity(32);
        values.extend([1, 2, 3]);
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::Ints(values)));
        let original_capacity = match &*list.borrow() {
            TypedVec::Ints(values) => values.capacity(),
            _ => unreachable!(),
        };
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(VmValue::List(Rc::clone(&list)));

        let mut tx = JitHeapTransactionGuard::begin();
        assert_eq!(
            rss_jit_list_set_int(JitCallCtx::active_token(), 0, 1, 99),
            0
        );
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(99)),
            "native helper should mutate during the transaction"
        );

        tx.abort();
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(2)),
            "abort should restore the pre-native list contents"
        );
        let mut restored = list.borrow_mut();
        let restored_capacity = match &*restored {
            TypedVec::Ints(values) => values.capacity(),
            _ => unreachable!(),
        };
        assert_eq!(restored_capacity, original_capacity);
        assert_eq!(
            restored.checked_push_accounted(VmValue::Int(4)),
            Ok(0),
            "interpreter replay must retain spare capacity and avoid a false growth charge",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_snapshot_refuses_inactive_call_context() {
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ])));

        JIT_CALL_CTX.with(|ctx| {
            assert_eq!(
                ctx.borrow().active_depth,
                0,
                "test should start outside a native context"
            );
        });
        assert!(
            !jit_snapshot_list_before_write(0, &list),
            "snapshot registration must fail closed outside a native call frame",
        );
        JIT_HEAP_WRITE_UNDO.with(|undo| {
            assert!(
                undo.borrow().is_empty(),
                "inactive snapshot attempts must not create undo entries",
            );
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_abort_restores_direct_flat_int_write() {
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
            VmValue::Int(3),
        ])));

        let mut tx = JitHeapTransactionGuard::begin();
        assert!(
            jit_snapshot_list_before_write(0, &list),
            "direct flat writes must be journaled inside a native transaction",
        );
        {
            let mut borrowed = list.borrow_mut();
            let slice = borrowed
                .as_ints_mut_slice()
                .expect("test list should use flat Int storage");
            assert_eq!(slice.len(), 3);
            slice[1] = 99;
        }
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(99)),
            "direct native write should mutate during the transaction"
        );

        tx.abort();
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(2)),
            "abort should restore direct flat-list writes"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_after_native_heap_write_falls_back_from_top() {
        let mut vm = empty_vm();
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );
        let func = Rc::new(native_test_function(
            "write_then_overflow",
            1,
            5,
            vec![
                RegInstr::LoadInt { dst: 1, value: 0 },
                RegInstr::LoadInt { dst: 2, value: 99 },
                RegInstr::ListSet {
                    dst: 3,
                    list: 0,
                    index: 1,
                    value: 2,
                },
                RegInstr::LoadInt {
                    dst: 4,
                    value: i64::MAX,
                },
                RegInstr::AddInt {
                    dst: 4,
                    lhs: 4,
                    rhs: 2,
                },
                RegInstr::Return { src: 4 },
            ],
        ));
        let list: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(1)])));

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::clone(&list)));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Fallback),
            "a guard bail after a native heap write must re-run from the top, not precise-resume",
        );
        assert_eq!(
            vm.frames.last().expect("frame").ip,
            0,
            "fallback path must leave the interpreter at the function entry",
        );
        assert_eq!(
            list.borrow().get(0),
            Some(VmValue::Int(1)),
            "heap transaction abort must restore the native write before fallback",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn flat_int_mut_alias_detects_list_hidden_inside_heap_input() {
        let list: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(1)])));
        let layout = native_test_layout("Box", &["items"]);
        let heap_input = VmValue::Struct(Rc::new(VmStruct::with_layout(
            layout,
            vec![VmValue::List(Rc::clone(&list))],
        )));
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(heap_input);

        assert!(
            jit_heap_inputs_alias_flat_mut(&[(0, 0)], &[Rc::clone(&list)]),
            "a handle input containing the same list Rc as a FlatIntMut arg must force fallback",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_string_from_int_return_allocates_heap_result() {
        let mut vm = empty_vm();
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(string_from_int_return_func());

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(42));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::String(value)) => assert_eq!(&*value, "42"),
            NativeAttempt::Completed(_) => panic!("expected native String(\"42\") completion"),
            NativeAttempt::Resumed => panic!("expected native completion, got Resumed"),
            NativeAttempt::Fallback => panic!("expected native completion, got Fallback"),
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.translated, 1,
            "function should translate to native IR"
        );
        assert_eq!(
            stats.native_calls, 1,
            "function should complete in native code"
        );
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "output table must be cleared after materialization"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_string_from_int_handle_feeds_string_len() {
        let mut vm = empty_vm();
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(string_from_int_len_func());

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(12345));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(value)) => assert_eq!(value, 5),
            NativeAttempt::Completed(_) => panic!("expected native Int length completion"),
            NativeAttempt::Resumed => panic!("expected native completion, got Resumed"),
            NativeAttempt::Fallback => panic!("expected native completion, got Fallback"),
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.translated, 1,
            "function should translate to native IR"
        );
        assert_eq!(
            stats.native_calls, 1,
            "function should complete in native code"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_string_concat_handle_feeds_string_len() {
        let mut vm = empty_vm();
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(string_concat_len_func());

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::string("item-"));
        vm.set_reg(1, VmValue::Int(42));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(value)) => assert_eq!(value, 7),
            NativeAttempt::Completed(_) => panic!("expected native Int length completion"),
            NativeAttempt::Resumed => panic!("expected native completion, got Resumed"),
            NativeAttempt::Fallback => panic!("expected native completion, got Fallback"),
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.translated, 1,
            "function should translate to native IR"
        );
        assert_eq!(
            stats.native_calls, 1,
            "function should complete in native code"
        );
        assert_eq!(
            stats.native_bails, 0,
            "concat helper should not force a native bail"
        );
    }

    /// §7.2 force-deopt twin: the SAME pass-through under the force-bail backend must
    /// `Fallback` (bail at entry) — NOT produce a heap result — and leave the output
    /// table empty. The interpreter re-run is then the sole source of the value, so
    /// the bailed native attempt has no observable effect (no leaked/double-
    /// materialized heap result). This is the mechanical proof of the §7.2 argument
    /// for the new return ABI.
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_result_force_deopt_leaves_output_table_empty() {
        let mut vm = empty_vm();
        // force_bail = true: pretend native bailed at its first guard (entry).
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, true, true).expect("native module"));
        let func = Rc::new(list_passthrough_func());

        let list: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(7)])));

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::clone(&list)));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Fallback),
            "force-deopt must bail (no heap result materialized), got a non-Fallback",
        );
        // The decisive §7.2 assertion: a bailed attempt leaves NO heap result behind.
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "a bailed attempt must leave the output table empty (no leaked result)",
            )
        });
    }

    /// Consecutive (not cumulative) semantics: a single successful native
    /// completion RESETS the bail counter, so a function that bails only
    /// intermittently keeps its native fast path. We drive `record_bail` /
    /// reset-on-success directly on the `NativeState` to prove the counter logic in
    /// isolation, since constructing an intermittently-bailing compiled function
    /// from scratch is far more fragile.
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_bail_counter_resets_on_success() {
        let mut native = NativeState::new(0, false, true).expect("native module");
        let function = 7;
        let key = NativeVersionKey {
            function,
            shape: ShapeKey::from_shapes([NativeParamShape::Int]),
        };
        let successful_key = NativeVersionKey {
            function,
            shape: ShapeKey::from_shapes([NativeParamShape::Bool]),
        };

        // Two consecutive bails — one short of the give-up threshold (3).
        native.record_bail(&key);
        native.record_bail(&key);
        assert_eq!(native.bail_counts.get(&key), Some(&2));

        // A success resets the counter (mirrors the `Some(bits)` arm in try_native).
        native.bail_counts.insert(key.clone(), 0);
        assert_eq!(native.bail_counts.get(&key), Some(&0));
        native.bail_counts.insert(successful_key.clone(), 0);

        // It now takes a full fresh run of `threshold` bails to disable this version.
        for _ in 0..NATIVE_BAIL_GIVEUP_THRESHOLD {
            native.record_bail(&key);
        }
        assert_eq!(
            native.cache.get(&key),
            Some(&None),
            "failed shape must be negative-cached",
        );
        assert_eq!(
            native.bail_counts.get(&successful_key),
            Some(&0),
            "one shape's failure must not alter another version's success state",
        );
    }
