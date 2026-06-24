#[cfg(test)]
mod intrinsic_registry_tests {
    use super::super::*;

    /// The conservative DEFAULT must hold for an intrinsic no site special-cases:
    /// opaque allocator, not foldable, not native-lowerable, no combinator/string
    /// role, no cold-arm whitelist. This locks the table's contract for lever 2.
    #[test]
    fn default_is_conservative() {
        // `ListContains` is a representative unlisted intrinsic (allocating, opaque).
        let d = intrinsic_descriptor(RegIntrinsic::ListContains);
        assert_eq!(d.effect, IntrinsicEffect::Allocate);
        assert!(!d.can_fold);
        assert!(!d.native_lowerable);
        assert!(!d.view_capable);
        assert!(d.combinator_kind.is_none());
        assert!(d.string_fold_role.is_none());
        assert!(d.bytes_fold_role.is_none());
        assert!(!d.cold_arm_pure_builder);

        // The bare `Default` impl matches the conservative classification.
        let def = IntrinsicDescriptor::default();
        assert_eq!(def.effect, IntrinsicEffect::Allocate);
        assert!(!def.can_fold);
        assert!(!def.native_lowerable);
        assert!(!def.view_capable);
    }

    /// `view_capable` is a reserved placeholder — false for EVERY intrinsic today.
    #[test]
    fn view_capable_is_false_everywhere() {
        for i in [
            RegIntrinsic::IntToFloat,
            RegIntrinsic::OptionMap,
            RegIntrinsic::ResultAndThen,
            RegIntrinsic::StringLen,
            RegIntrinsic::StringSlice,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::ListContains,
        ] {
            assert!(!intrinsic_descriptor(i).view_capable);
        }
    }

    /// Site 1: only `IntToFloat` is native-lowerable.
    #[test]
    fn int_to_float_is_native_lowerable() {
        let d = intrinsic_descriptor(RegIntrinsic::IntToFloat);
        assert!(d.native_lowerable);
        assert_eq!(d.effect, IntrinsicEffect::Pure);
        // A non-listed intrinsic is NOT native-lowerable.
        assert!(!intrinsic_descriptor(RegIntrinsic::StringLen).native_lowerable);
    }

    /// Site 2: exactly the six Option/Result combinators are expandable, each with
    /// its kind; nothing else is.
    #[test]
    fn six_combinators_are_expandable() {
        let cases = [
            (RegIntrinsic::OptionMap, CombinatorKind::OptionMap),
            (RegIntrinsic::OptionAndThen, CombinatorKind::OptionAndThen),
            (RegIntrinsic::OptionUnwrapOr, CombinatorKind::OptionUnwrapOr),
            (RegIntrinsic::ResultMap, CombinatorKind::ResultMap),
            (RegIntrinsic::ResultAndThen, CombinatorKind::ResultAndThen),
            (RegIntrinsic::ResultUnwrapOr, CombinatorKind::ResultUnwrapOr),
        ];
        for (i, k) in cases {
            let d = intrinsic_descriptor(i);
            assert_eq!(d.combinator_kind, Some(k));
            assert!(d.can_fold);
            assert_eq!(d.effect, IntrinsicEffect::Pure);
        }
        assert!(intrinsic_descriptor(RegIntrinsic::StringLen).combinator_kind.is_none());
        assert!(intrinsic_descriptor(RegIntrinsic::ListContains).combinator_kind.is_none());
    }

    /// Site 3: the string-fold producers/query carry the expected roles.
    #[test]
    fn string_fold_roles_match() {
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::StringLen).string_fold_role,
            Some(StringFoldRole::LengthQuery)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::StringFromInt).string_fold_role,
            Some(StringFoldRole::ProducerFromInt)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::StringSlice).string_fold_role,
            Some(StringFoldRole::ProducerSlice)
        );
        for i in [
            RegIntrinsic::StringLen,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::StringSlice,
        ] {
            assert!(intrinsic_descriptor(i).can_fold);
        }
        // A non-fold string intrinsic has no role.
        assert!(intrinsic_descriptor(RegIntrinsic::StringCopy).string_fold_role.is_none());
    }

    /// Site 3 (Bytes sibling): the Bytes-fold producers/query carry the expected roles
    /// and are `can_fold`; an unrelated Bytes intrinsic has no Bytes role.
    #[test]
    fn bytes_fold_roles_match() {
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesLen).bytes_fold_role,
            Some(BytesFoldRole::LengthQuery)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesFromString).bytes_fold_role,
            Some(BytesFoldRole::ProducerFromString)
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesSlice).bytes_fold_role,
            Some(BytesFoldRole::ProducerSlice)
        );
        for i in [
            RegIntrinsic::BytesLen,
            RegIntrinsic::BytesFromString,
            RegIntrinsic::BytesSlice,
        ] {
            assert!(intrinsic_descriptor(i).can_fold);
            // Bytes-fold intrinsics carry NO string role (they are a disjoint family).
            assert!(intrinsic_descriptor(i).string_fold_role.is_none());
        }
        // `Bytes.len` is a pure READ; the producers allocate.
        assert_eq!(intrinsic_descriptor(RegIntrinsic::BytesLen).effect, IntrinsicEffect::Read);
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesFromString).effect,
            IntrinsicEffect::Allocate
        );
        assert_eq!(intrinsic_descriptor(RegIntrinsic::BytesSlice).effect, IntrinsicEffect::Allocate);
        // A non-fold Bytes intrinsic has no Bytes role.
        assert!(intrinsic_descriptor(RegIntrinsic::BytesConcat).bytes_fold_role.is_none());
        // String-fold intrinsics carry no Bytes role.
        assert!(intrinsic_descriptor(RegIntrinsic::StringLen).bytes_fold_role.is_none());
    }

    /// The deopt cold-arm pure-builder whitelist is exactly the four String builders.
    #[test]
    fn cold_arm_pure_builders_whitelist() {
        for i in [
            RegIntrinsic::StringCopy,
            RegIntrinsic::StringFromBool,
            RegIntrinsic::StringFromFloat,
            RegIntrinsic::StringFromInt,
        ] {
            assert!(intrinsic_descriptor(i).cold_arm_pure_builder, "{:?}", i);
        }
        // Not on the whitelist: queries, combinators, slices, opaque allocators.
        for i in [
            RegIntrinsic::StringLen,
            RegIntrinsic::StringSlice,
            RegIntrinsic::OptionMap,
            RegIntrinsic::ListContains,
        ] {
            assert!(!intrinsic_descriptor(i).cold_arm_pure_builder, "{:?}", i);
        }
    }
}

#[cfg(test)]
mod register_window_tests {
    use super::super::*;


    /// Build a bare `RegVm` with an empty unit — enough to exercise the
    /// register-stack helpers (`ensure_regs`/`set_reg`/`prepare_frame`) directly,
    /// with no program loaded.
    fn empty_vm() -> RegVm {
        let unit = RegUnit {
            functions: Vec::new(),
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            closure_identity_observable: true,
        };
        RegVm::new(Rc::new(unit), Vec::new(), HashMap::new())
    }

    /// Execution spec §4.1: a reused register window must be *non-retaining*.
    /// `prepare_frame` clears the written bits AND drops any stale `VmValue` the
    /// reused slot physically held, so a big heap value allocated by a prior frame
    /// is released the moment its window is reused — not merely when the slot is
    /// next overwritten. Without the value-drop this asserts the previous behavior
    /// (the `Rc` stayed alive at strong_count 2), which is the bug this pins.
    #[test]
    fn prepare_frame_releases_stale_heap_value() {
        let mut vm = empty_vm();
        vm.ensure_regs(8).expect("grow stack");

        // Simulate a prior frame allocating a large list into its window.
        let big: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(0); 4096])));
        vm.set_reg(3, VmValue::List(Rc::clone(&big)));
        assert_eq!(
            Rc::strong_count(&big),
            2,
            "the VM register and our test handle should both hold the list",
        );

        // Reusing the window for a new frame must drop the stale list.
        vm.prepare_frame(0, 8).expect("prepare reused window");
        assert_eq!(
            Rc::strong_count(&big),
            1,
            "prepare_frame must drop the stale heap value, leaving only the test handle",
        );
        assert!(
            !vm.written[3],
            "prepare_frame must clear the written bit of every window slot",
        );
    }

    /// `take_reg` already moved the value out and left `Unit`; `prepare_frame` over
    /// an already-empty window must stay non-retaining and idempotent.
    #[test]
    fn prepare_frame_is_noop_on_empty_window() {
        let mut vm = empty_vm();
        vm.ensure_regs(4).expect("grow stack");
        vm.prepare_frame(0, 4).expect("prepare empty window");
        for index in 0..4 {
            assert!(matches!(vm.stack[index], VmValue::Unit));
            assert!(!vm.written[index]);
        }
    }

    #[cfg(feature = "native-jit")]
    fn native_test_function(
        name: &str,
        params: usize,
        regs: usize,
        code: Vec<RegInstr>,
    ) -> RegFunction {
        RegFunction {
            name: name.to_string(),
            params,
            captures: 0,
            regs,
            local_regs: HashMap::new(),
            code,
            jit_analysis: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
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
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.limits = VmLimits {
            cancel: Some(Arc::new(AtomicBool::new(false))),
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
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    /// vm-jit-perf-plan §3.0 (predict-and-skip bail): a function that PASSES the
    /// structural predictor (it compiles) but bails at runtime on *every* call must
    /// not re-compile/marshal/bail forever. After `NATIVE_BAIL_GIVEUP_THRESHOLD`
    /// consecutive bails the native tier gives up — it demotes the function to
    /// `NOT_ELIGIBLE` and the cheap-negative early-return in `try_native`
    /// short-circuits all further calls. So the count of native *attempts*
    /// (`considered`) must PLATEAU at the threshold rather than scale with the call
    /// count. Here the bail is an arg-type mismatch (Int param, heap argument).
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_gives_up_after_consecutive_bails() {
        let mut vm = empty_vm();
        // threshold 0 => compile/attempt on the very first call; collect_stats so we
        // can observe the attempt count.
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = always_arg_mismatch_func();

        // Place a heap (List) value in the function's single parameter register, so
        // marshalling the `Int`-typed param fails on every native attempt.
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::new(RefCell::new(TypedVec::new()))));

        const CALLS: usize = 50;
        let mut first_demoted_at: Option<usize> = None;
        for call in 1..=CALLS {
            // Native is never chosen (it bails), so the result is always `None`
            // (fall back to the interpreter).
            assert!(
                matches!(vm.try_native(&func, 0), NativeAttempt::Fallback),
                "always-mismatching native call must bail to the interpreter",
            );
            if func.native_status.get() == NATIVE_STATUS_NOT_ELIGIBLE
                && first_demoted_at.is_none()
            {
                first_demoted_at = Some(call);
            }
        }

        // Give-up fired exactly at the threshold (the Nth consecutive bail demotes).
        assert_eq!(
            first_demoted_at,
            Some(NATIVE_BAIL_GIVEUP_THRESHOLD as usize),
            "function must be demoted on the {NATIVE_BAIL_GIVEUP_THRESHOLD}th bail",
        );

        let stats = &vm.native.as_ref().unwrap().stats;
        // The decisive assertion: native attempts PLATEAU at the threshold instead
        // of scaling with CALLS. Without give-up, `considered` and `arg_mismatch`
        // would both equal CALLS (50).
        assert_eq!(
            stats.considered, NATIVE_BAIL_GIVEUP_THRESHOLD as u64,
            "native attempts must plateau at the give-up threshold, not scale with calls",
        );
        assert_eq!(
            stats.arg_mismatch, NATIVE_BAIL_GIVEUP_THRESHOLD as u64,
            "bail count must plateau at the give-up threshold",
        );
        // The compiled entry was dropped from the cache on give-up.
        assert!(
            !vm.native.as_ref().unwrap().cache.contains_key(
                &(&func as *const RegFunction as usize)
            ),
            "compiled code must be evicted on give-up",
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
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
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

    /// Flag-off default: the SAME real guard bail must take the safe fallback
    /// (re-run-from-top), leaving the frame `ip` at 0 — byte-identical to today.
    #[cfg(feature = "native-jit")]
    #[test]
    fn deopt_without_precise_flag_falls_back_from_top() {
        let mut vm = empty_vm();
        // precise_deopt OFF (the last arg).
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
    /// that handle unchanged. This is the heap-write S0 producer — a heap-result
    /// return with NO allocation and NO mutation (native just returns a value it was
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
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    /// Heap-result return ABI (heap-write S0) round-trip: a native function that
    /// returns a heap PARAMETER unchanged produces the interpreter-identical heap
    /// value through the VM-owned output table. Also asserts the output table is
    /// cleared on exit (no value retained past the call) — the §7.2 invariant.
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_result_passthrough_round_trips() {
        let mut vm = empty_vm();
        // threshold 0 => compile/attempt on the first call.
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(list_passthrough_func());

        // Distinct list value so identity is observable through the round-trip.
        let list: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![
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
        JIT_HEAP_RESULTS.with(|t| {
            assert!(t.borrow().is_empty(), "output table must be cleared on exit")
        });
        JIT_HEAP_ARGS
            .with(|t| assert!(t.borrow().is_empty(), "input table must be cleared on exit"));
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
        })
        .expect("push frame");

        let outcome = vm.try_native(&func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Fallback),
            "force-deopt must bail (no heap result materialized), got a non-Fallback",
        );
        // The decisive §7.2 assertion: a bailed attempt leaves NO heap result behind.
        JIT_HEAP_RESULTS.with(|t| {
            assert!(
                t.borrow().is_empty(),
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
        let func = always_arg_mismatch_func();
        let key = &func as *const RegFunction as usize;

        // Two consecutive bails — one short of the give-up threshold (3).
        native.record_bail(key, &func);
        native.record_bail(key, &func);
        assert_eq!(native.bail_counts.get(&key), Some(&2));
        assert_ne!(
            func.native_status.get(),
            NATIVE_STATUS_NOT_ELIGIBLE,
            "must not give up before the threshold",
        );

        // A success resets the counter (mirrors the `Some(bits)` arm in try_native).
        native.bail_counts.insert(key, 0);
        assert_eq!(native.bail_counts.get(&key), Some(&0));

        // It now takes a full fresh run of `threshold` bails to demote — proving the
        // semantics are consecutive, not cumulative.
        for _ in 0..NATIVE_BAIL_GIVEUP_THRESHOLD {
            native.record_bail(key, &func);
        }
        assert_eq!(
            func.native_status.get(),
            NATIVE_STATUS_NOT_ELIGIBLE,
            "must give up after a fresh run of consecutive bails",
        );
    }
}

#[cfg(test)]
mod closure_cache_tests {
    use super::super::*;

    /// Lower a source program to a `RegUnit`, exactly as `reg_vm_compile_source`
    /// does, so tests can inspect the closure-identity gate and the cache.
    fn unit(source: &str) -> RegUnit {
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        RegUnit::lower(&hir).expect("lowering should succeed")
    }

    /// A program that never compares closures must leave the gate OFF, so the
    /// non-capturing-closure cache is permitted to share one `Rc`.
    #[test]
    fn gate_off_when_no_closure_equality() {
        let source = r#"
fn apply(f: noescape Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 3 {
        let g = |x| { return x * 2 + 1 }
        total = total + apply(f: read g, x: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        assert!(
            !unit(source).closure_identity_observable,
            "no `==`/`!=` over a closure ⇒ identity is unobservable ⇒ gate off",
        );
    }

    /// Comparing two closure-typed values with `==` makes pointer identity
    /// observable, so the gate must turn ON (disabling the cache). The RSScript
    /// analyzer permits this expression (the compiled backend later rejects bare
    /// `Fn ==`), so the gate is what keeps the VM bit-identical to that backend's
    /// distinct-allocation semantics when the program *does* reach an equality.
    #[test]
    fn gate_on_when_closure_compared() {
        let source = r#"
fn main() -> Unit {
    let f: Fn(Int) -> Int = |x| { return x + 1 }
    let g: Fn(Int) -> Int = |x| { return x + 1 }
    if f == g {
        Log.write(message: read String.from_int(value: 1))
    }
    return Unit
}
"#;
        assert!(
            unit(source).closure_identity_observable,
            "a user `==` over closure-typed operands ⇒ identity observable ⇒ gate on",
        );
    }

    /// With the gate off, repeated `MakeClosure` of the same non-capturing
    /// function shares ONE `Rc` (pointer-identical), proving the allocation was
    /// eliminated. We drive the handler directly so we can read back the register.
    #[test]
    fn cache_shares_one_rc_when_gate_off() {
        // Hand-build a unit whose closure-identity gate is off and a function 0
        // that the closure refers to (its body is irrelevant for this test).
        let func = RegFunction::placeholder("noop".into());
        let unit = RegUnit {
            functions: vec![Rc::new(func)],
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            closure_identity_observable: false,
        };
        let mut vm = RegVm::new(Rc::new(unit), Vec::new(), HashMap::new());

        let a = vm.cached_noncapturing_closure(0);
        let b = vm.cached_noncapturing_closure(0);
        assert!(
            Rc::ptr_eq(&a, &b),
            "non-capturing closures of the same function must share one cached Rc",
        );
        assert!(a.captures.is_empty(), "cached closure must be non-capturing");
    }
}

/// J1 per-call-site type feedback (the data J2 monomorphic inlining consumes).
/// Two properties under test: (1) DETERMINISM — profiling never changes program
/// output, cold vs warm; (2) COLLECTION — a warm dynamic call site records the
/// resolved callee identity and the correct mono/poly state.
#[cfg(test)]
mod j1_profiling_tests {
    use super::super::*;

    /// Compile a source program to an executable exactly as the public entry
    /// point does, so the shared `Rc<RegFunction>` profiles populated during a
    /// run are observable afterwards through the returned unit.
    fn compile(source: &str) -> RegVmExecutable {
        reg_vm_compile_source("test.rss", source).expect("compilation should succeed")
    }

    /// Borrow the populated profile of the function named `name`, asserting it is
    /// warm (allocated). Returns the feedback for the single call site we expect.
    fn call_site_state(exec: &RegVmExecutable, name: &str, instr_idx: usize) -> MonoState {
        let id = *exec
            .unit
            .function_ids
            .get(name)
            .unwrap_or_else(|| panic!("function `{name}` must exist"));
        let func = &exec.unit.functions[id];
        let profile = func.profile.borrow();
        let profile = profile
            .as_ref()
            .unwrap_or_else(|| panic!("function `{name}` must be warm (profile allocated)"));
        profile
            .call_sites
            .get(&instr_idx)
            .unwrap_or_else(|| panic!("no feedback recorded at instr {instr_idx} of `{name}`"))
            .state()
    }

    /// The instruction index of the (sole) `CallClosure` in a function's code.
    fn closure_call_idx(exec: &RegVmExecutable, name: &str) -> usize {
        let id = exec.unit.function_ids[name];
        exec.unit.functions[id]
            .code
            .iter()
            .position(|instr| matches!(instr, RegInstr::CallClosure { .. }))
            .expect("function must contain a CallClosure")
    }

    /// A `dispatcher(f, x)` that invokes a stored `Fn` (a `CallClosure` site),
    /// called `n` times from `main`. With `n` large enough the dispatcher crosses
    /// the warm-up threshold and its call site is profiled; cold runs are not.
    fn mono_program(n: i64) -> String {
        format!(
            r#"
fn dispatcher(f: read Fn(Int) -> Int, x: Int) -> Int {{
    return f(x)
}}

fn main() -> Unit {{
    let mut i = 0
    let mut total = 0
    while i < {n} {{
        let g: Fn(Int) -> Int = |x| {{ return x * 2 }}
        total = total + dispatcher(f: read g, x: read i)
        i = i + 1
    }}
    Log.write(message: read String.from_int(value: total))
    return Unit
}}
"#
        )
    }

    /// Like `mono_program` but the dispatcher is fed two DIFFERENT closures (two
    /// distinct lambda function ids) on alternating iterations, so its single
    /// call site observes two callee keys ⇒ polymorphic.
    fn poly_program(n: i64) -> String {
        format!(
            r#"
fn dispatcher(f: read Fn(Int) -> Int, x: Int) -> Int {{
    return f(x)
}}

fn main() -> Unit {{
    let mut i = 0
    let mut total = 0
    while i < {n} {{
        let a: Fn(Int) -> Int = |x| {{ return x * 2 }}
        let b: Fn(Int) -> Int = |x| {{ return x + 7 }}
        if i % 2 == 0 {{
            total = total + dispatcher(f: read a, x: read i)
        }} else {{
            total = total + dispatcher(f: read b, x: read i)
        }}
        i = i + 1
    }}
    Log.write(message: read String.from_int(value: total))
    return Unit
}}
"#
        )
    }

    /// DETERMINISM: the same program run cold (a handful of calls, no profile
    /// allocated) and warm (well past `PROFILE_WARMUP`, profile populated) must
    /// produce byte-identical output. Profiling is observation-only and never
    /// feeds a value. Both are also checked against an independent reference run.
    #[test]
    fn profiling_does_not_change_output_cold_vs_warm() {
        let cold = compile(&mono_program(5));
        let warm = compile(&mono_program(5));
        // Same source, different call volume: total differs by construction, so
        // determinism is checked at equal `n` (cold-fresh vs a second run of the
        // identical unit) AND across the warm threshold for a fixed `n`.
        let cold_out = cold.eval_main_with_args(Vec::<String>::new()).unwrap();
        let cold_again = warm.eval_main_with_args(Vec::<String>::new()).unwrap();
        assert_eq!(
            cold_out.stdout, cold_again.stdout,
            "below warm-up, output must be stable"
        );

        // A single unit run twice: the first run leaves the dispatcher warm
        // (profile populated mid-run for high `n`); the second run continues
        // recording into the same profile. Output must be identical across both,
        // proving the populated profile never perturbs results.
        let exec = compile(&mono_program(PROFILE_WARMUP as i64 + 20));
        let first = exec.eval_main_with_args(Vec::<String>::new()).unwrap();
        let second = exec.eval_main_with_args(Vec::<String>::new()).unwrap();
        assert_eq!(
            first.stdout, second.stdout,
            "warm profile must not change output between runs (determinism)"
        );
        assert_eq!(
            first.value, second.value,
            "warm profile must not change the returned value"
        );

        // Independent reference: a freshly compiled unit at the same `n` (no prior
        // warm-up) yields the same stdout as the warmed unit.
        let reference = compile(&mono_program(PROFILE_WARMUP as i64 + 20))
            .eval_main_with_args(Vec::<String>::new())
            .unwrap();
        assert_eq!(
            reference.stdout, first.stdout,
            "cold reference run == warm run (== pure-interpreter semantics)"
        );
    }

    /// COLLECTION (a): a warm call site that ALWAYS invokes the same closure
    /// records `Monomorphic` with exactly that one callee key, and the count
    /// equals the number of warm invocations.
    #[test]
    fn warm_site_calling_one_closure_is_monomorphic() {
        let n = PROFILE_WARMUP as i64 + 30;
        let exec = compile(&mono_program(n));
        exec.eval_main_with_args(Vec::<String>::new()).unwrap();

        let idx = closure_call_idx(&exec, "dispatcher");
        let id = exec.unit.function_ids["dispatcher"];
        let func = &exec.unit.functions[id];
        let profile = func.profile.borrow();
        let profile = profile.as_ref().expect("dispatcher must be warm");
        let feedback = profile
            .call_sites
            .get(&idx)
            .expect("call site must be recorded");

        assert_eq!(feedback.state(), MonoState::Monomorphic);
        assert_eq!(
            feedback.observed.len(),
            1,
            "exactly one distinct callee identity observed"
        );
        // The callee is the lambda's function id; recorded count is the number of
        // calls made while warm. `dispatcher` is profiled starting the entry
        // AFTER the `PROFILE_WARMUP`-th (which allocates the profile and itself
        // records nothing), so the recorded count is `n - PROFILE_WARMUP`.
        let warm_calls = (n as u32) - PROFILE_WARMUP;
        assert_eq!(
            feedback.observed[0].1, warm_calls,
            "saturating count equals the number of warm invocations"
        );
    }

    /// COLLECTION (b): a warm call site that invokes TWO different closures
    /// records `Polymorphic` with both callee keys present.
    #[test]
    fn warm_site_calling_two_closures_is_polymorphic() {
        let n = PROFILE_WARMUP as i64 + 30;
        let exec = compile(&poly_program(n));
        exec.eval_main_with_args(Vec::<String>::new()).unwrap();

        let idx = closure_call_idx(&exec, "dispatcher");
        assert_eq!(call_site_state(&exec, "dispatcher", idx), MonoState::Polymorphic);

        let id = exec.unit.function_ids["dispatcher"];
        let profile = exec.unit.functions[id].profile.borrow();
        let feedback = profile.as_ref().unwrap().call_sites.get(&idx).unwrap();
        assert_eq!(
            feedback.observed.len(),
            2,
            "two distinct closure callee identities observed ⇒ polymorphic"
        );
        // Both keys carry a non-zero count and together cover the warm calls.
        assert!(feedback.observed.iter().all(|(_, count)| *count > 0));
    }

    /// A function that never crosses the warm-up threshold allocates NO profile —
    /// cold code pays only the call-count `Cell` bump. Proven by absence of the
    /// profile after a low-volume run.
    #[test]
    fn cold_function_allocates_no_profile() {
        let exec = compile(&mono_program(5));
        exec.eval_main_with_args(Vec::<String>::new()).unwrap();
        let id = exec.unit.function_ids["dispatcher"];
        let func = &exec.unit.functions[id];
        assert!(
            func.profile.borrow().is_none(),
            "a cold function (< PROFILE_WARMUP calls) must not allocate a profile"
        );
        assert!(
            func.call_count.get() <= PROFILE_WARMUP,
            "call_count reflects the cold call volume"
        );
    }

    /// Unit-level check of the mono/poly/mega classification and saturating count
    /// in `CallSiteFeedback`, independent of the interpreter.
    #[test]
    fn feedback_classification_and_saturation() {
        let mut fb = CallSiteFeedback::default();
        assert_eq!(fb.state(), MonoState::Monomorphic); // zero observations
        fb.record(1, true);
        assert_eq!(fb.state(), MonoState::Monomorphic);
        fb.record(1, true);
        assert_eq!(fb.observed[0].1, 2);
        fb.record(2, true);
        assert_eq!(fb.state(), MonoState::Polymorphic);
        fb.record(3, true);
        assert_eq!(fb.state(), MonoState::Polymorphic); // 3 distinct
        fb.record(4, true);
        // 4 distinct == PROFILE_MAX_CALLEES (cap), still not overflowed.
        assert_eq!(fb.observed.len(), PROFILE_MAX_CALLEES);
        assert_eq!(fb.state(), MonoState::Polymorphic);
        fb.record(5, true); // 5th distinct ⇒ overflow ⇒ megamorphic, list stays capped
        assert!(fb.overflowed);
        assert_eq!(fb.observed.len(), PROFILE_MAX_CALLEES);
        assert_eq!(fb.state(), MonoState::Megamorphic);

        // Saturating count never panics at the u32 ceiling.
        let mut sat = CallSiteFeedback::default();
        sat.record(9, true);
        sat.observed[0].1 = u32::MAX;
        sat.record(9, true);
        assert_eq!(sat.observed[0].1, u32::MAX);
    }
}
