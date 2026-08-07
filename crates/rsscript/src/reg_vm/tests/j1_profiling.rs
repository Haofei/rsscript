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
    Output.write(message: read String.from_int(value: total))
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
    Output.write(message: read String.from_int(value: total))
    return Unit
}}
"#
        )
    }

    #[cfg(feature = "native-jit")]
    fn branch_program(n: i64) -> String {
        format!(
            r#"
fn main() -> Unit {{
    let mut i = 0
    let mut total = 0
    while i < {n} {{
        if i % 4 == 0 {{
            total = total + 10
        }} else {{
            total = total + 1
        }}
        i = i + 1
    }}
    Output.write(message: read String.from_int(value: total))
    return Unit
}}
"#
        )
    }

    /// Three-callee profile where first-seen order in the sampling window is B, A,
    /// C but frequency order is A, B, C. This exercises profile-guided PIC arm
    /// ordering independently of semantic correctness.
    #[cfg(feature = "native-jit")]
    fn weighted_poly_program(n: i64) -> String {
        format!(
            r#"
fn dispatcher(f: read Fn(Int) -> Int, x: Int) -> Int {{
    return f(x)
}}

fn main() -> Unit {{
    let mut i = 0
    let mut total = 0
    while i < {n} {{
        if i % 6 == 0 {{
            let c: Fn(Int) -> Int = |x| {{ return 0 - x }}
            total = total + dispatcher(f: read c, x: read i)
        }} else if i % 6 < 3 {{
            let b: Fn(Int) -> Int = |x| {{ return x + 7 }}
            total = total + dispatcher(f: read b, x: read i)
        }} else {{
            let a: Fn(Int) -> Int = |x| {{ return x * 2 - 1 }}
            total = total + dispatcher(f: read a, x: read i)
        }}
        i = i + 1
    }}
    Output.write(message: read String.from_int(value: total))
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
        assert_eq!(
            call_site_state(&exec, "dispatcher", idx),
            MonoState::Polymorphic
        );

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

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_warm_branch_site_records_taken_and_fallthrough_counts() {
        let n = PROFILE_WARMUP as i64 + 40;
        let exec = compile(&branch_program(n));
        let native = exec
            .eval_main_with_args_native(Vec::<String>::new())
            .expect("native-enabled run should execute branch fixture");
        let interp = compile(&branch_program(n))
            .eval_main_with_args(Vec::<String>::new())
            .expect("interpreter run should execute branch fixture");
        assert_eq!(
            native.stdout, interp.stdout,
            "branch profiling is observation-only and must not change output"
        );

        let id = exec.unit.function_ids["main"];
        let func = &exec.unit.functions[id];
        let profile = func.profile.borrow();
        let profile = profile
            .as_ref()
            .expect("native-enabled branch-heavy function should allocate a profile");
        let mixed_branch = profile.branch_feedback_sites().find(|(ip, feedback)| {
            matches!(
                func.code[*ip],
                RegInstr::JumpIfBool { .. } | RegInstr::JumpIfIntCompare { .. }
            ) && feedback.taken > 0
                && feedback.fallthrough > 0
                && profile.branch_bias(*ip) == BranchBias::Mixed
        });
        assert!(
            mixed_branch.is_some(),
            "expected at least one conditional branch with mixed taken/fallthrough feedback; profile={profile:#?}; code={:#?}",
            func.code,
        );
    }

    #[test]
    fn branch_feedback_bias_requires_enough_strong_samples() {
        let mut feedback = BranchFeedback::default();
        assert_eq!(feedback.bias(), BranchBias::NoSamples);
        assert_eq!(feedback.hot_edge(), None);

        for _ in 0..(PROFILE_BRANCH_MIN_SAMPLES - 1) {
            feedback.record(true);
        }
        assert_eq!(feedback.bias(), BranchBias::UnderSampled);
        assert_eq!(feedback.hot_edge(), None);

        feedback.record(true);
        assert_eq!(feedback.bias(), BranchBias::TakenHot);
        assert_eq!(feedback.hot_edge(), Some(true));

        let mut mixed = BranchFeedback::default();
        for _ in 0..12 {
            mixed.record(true);
        }
        for _ in 0..8 {
            mixed.record(false);
        }
        assert_eq!(mixed.bias(), BranchBias::Mixed);
        assert_eq!(mixed.hot_edge(), None);

        let mut fallthrough = BranchFeedback::default();
        for _ in 0..PROFILE_BRANCH_MIN_SAMPLES {
            fallthrough.record(false);
        }
        assert_eq!(fallthrough.bias(), BranchBias::FallthroughHot);
        assert_eq!(fallthrough.hot_edge(), Some(false));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_marks_profile_cold_branch_blocks() {
        let source = r#"
fn hot(x: Int) -> Int {
    if x < 1000 {
        return x + 1
    }
    return 0
}

fn main() -> Unit {
    return Unit
}
"#;
        let exec = compile(source);
        let id = exec.unit.function_ids["hot"];
        let func = &exec.unit.functions[id];
        let (branch_ip, target) =
            func.code
                .iter()
                .enumerate()
                .find_map(|(ip, instr)| match instr {
                    RegInstr::JumpIfBool { target, .. }
                    | RegInstr::JumpIfIntCompare { target, .. } => Some((ip, *target)),
                    _ => None,
                })
                .expect("fixture should lower to a conditional branch");

        {
            let mut profile = func.profile.borrow_mut();
            let profile = profile.get_or_insert_with(|| Box::new(FunctionProfile::default()));
            profile.branch_sites.insert(
                branch_ip,
                BranchFeedback {
                    taken: 1,
                    fallthrough: PROFILE_BRANCH_MIN_SAMPLES,
                },
            );
        }
        let report_lines = jit_profile_report_lines(&exec.unit, func).join("\n");
        assert!(
            report_lines.contains("hot_edge=fallthrough")
                && report_lines.contains("side_exit_candidate=target"),
            "strong branch bias should be visible in the JIT report; lines:\n{report_lines}",
        );

        let (jit_fn, ..) =
            translate_to_native_jit(&exec.unit, func).expect("profiled fixture should translate");
        assert!(
            jit_fn.cold_blocks.contains(&(target as u32)),
            "strong fallthrough-hot profile should mark the explicit branch target cold; cold_blocks={:?}; code={:#?}",
            jit_fn.cold_blocks,
            func.code,
        );
        assert!(
            jit_fn.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::ProfiledJumpIfBool {
                    hot_target: false,
                    ..
                } | vm_jit::JitInstr::ProfiledJumpIfIntCompare {
                    hot_target: false,
                    ..
                }
            )),
            "strong fallthrough-hot profile should lower the branch to a profiled side exit; jit={:#?}",
            jit_fn.code,
        );

        let mut vm = RegVm::new(Rc::clone(&exec.unit), Vec::new(), HashMap::new());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, false, false, false)
                .expect("native module"),
        );
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(1));
        vm.push_frame(Frame {
            func: Rc::clone(func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .expect("push frame");
        let outcome = vm.try_native(func, 0);
        assert!(
            matches!(outcome, NativeAttempt::Completed(VmValue::Int(2))),
            "profiled fixture should run to native completion",
        );
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.profile_branch_cold_blocks > 0,
            "native compile stats should expose profile-driven cold blocks: {stats:?}",
        );
        assert!(
            stats.profile_branch_side_exits > 0,
            "native compile stats should expose profile-driven branch side exits: {stats:?}",
        );
        assert_eq!(
            stats.to_json()["profile_branch_cold_blocks"].as_u64(),
            Some(stats.profile_branch_cold_blocks),
            "bench JSON should expose profile-driven cold blocks",
        );
        assert_eq!(
            stats.to_json()["profile_branch_side_exits"].as_u64(),
            Some(stats.profile_branch_side_exits),
            "bench JSON should expose profile-driven branch side exits",
        );
        assert!(
            stats.summary().contains("profile_branch_cold_blocks="),
            "text summary should expose profile-driven cold blocks: {}",
            stats.summary(),
        );
        assert!(
            stats.summary().contains("profile_branch_side_exits="),
            "text summary should expose profile-driven branch side exits: {}",
            stats.summary(),
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_translation_uses_profile_hot_branch_edges() {
        let code = vec![
            RegInstr::JumpIfIntCompare {
                lhs: 0,
                rhs: 1,
                op: RegIntCompare::Less,
                expected: false,
                target: 7,
            },
            RegInstr::JumpIfIntCompare {
                lhs: 0,
                rhs: 4,
                op: RegIntCompare::Less,
                expected: true,
                target: 4,
            },
            RegInstr::AddInt {
                dst: 2,
                lhs: 2,
                rhs: 0,
            },
            RegInstr::Jump { target: 5 },
            RegInstr::SubInt {
                dst: 2,
                lhs: 2,
                rhs: 0,
            },
            RegInstr::AddInt {
                dst: 0,
                lhs: 0,
                rhs: 3,
            },
            RegInstr::Jump { target: 0 },
            RegInstr::Return { src: 2 },
        ];
        let mut profile = FunctionProfile::default();
        profile.branch_sites.insert(
            0,
            BranchFeedback {
                taken: 1,
                fallthrough: PROFILE_BRANCH_MIN_SAMPLES,
            },
        );
        profile.branch_sites.insert(
            1,
            BranchFeedback {
                taken: 1,
                fallthrough: PROFILE_BRANCH_MIN_SAMPLES,
            },
        );
        let func = RegFunction {
            name: "profiled_osr".to_string(),
            params: 5,
            captures: 0,
            regs: 5,
            local_regs: HashMap::new(),
            code: code.clone(),
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(Some(Box::new(profile))),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        };
        let lp = OsrLoop { header: 0, exit: 7 };
        let ip_map: Vec<usize> = (0..code.len()).collect();

        let (jit, _, _, _, _, _, _) = translate_osr_loop_profiled(
            &func,
            &code,
            5,
            func.params,
            func.captures,
            lp,
            &ip_map,
            &[],
            &[],
        )
        .expect("profiled scalar loop should translate to OSR native IR");

        assert!(
            jit.cold_blocks.contains(&(lp.exit as u32)),
            "fallthrough-hot OSR profile should mark the OSR-exit block cold; cold_blocks={:?}",
            jit.cold_blocks,
        );
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::ProfiledJumpIfIntCompare {
                    target: 4,
                    hot_target: false,
                    ..
                }
            )),
            "fallthrough-hot OSR profile should lower in-loop cold branches to profiled side exits; jit={:#?}",
            jit.code,
        );
        assert!(
            matches!(
                jit.code.first(),
                Some(vm_jit::JitInstr::JumpIfIntCompare { target: 7, .. })
            ),
            "OSR loop-header exit must stay a real branch to OsrExit instead of deopting at the header; jit={:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn polymorphic_inline_targets_are_ordered_hottest_first() {
        let exec = compile(&weighted_poly_program(PROFILE_RECORD_LIMIT as i64 + 40));
        exec.eval_main_with_args(Vec::<String>::new()).unwrap();

        let idx = closure_call_idx(&exec, "dispatcher");
        let id = exec.unit.function_ids["dispatcher"];
        let func = &exec.unit.functions[id];
        assert_eq!(
            func.call_count.get(),
            PROFILE_RECORD_LIMIT,
            "profile must be frozen before native PIC target selection"
        );

        let profile = func.profile.borrow();
        let feedback = profile.as_ref().unwrap().call_sites.get(&idx).unwrap();
        assert_eq!(feedback.state(), MonoState::Polymorphic);
        assert_eq!(feedback.observed.len(), 3);

        let first_seen: Vec<usize> = feedback
            .observed
            .iter()
            .map(|(key, _)| *key as usize)
            .collect();
        let mut expected: Vec<(usize, u32, usize)> = feedback
            .observed
            .iter()
            .enumerate()
            .map(|(first_seen, (key, count))| (*key as usize, *count, first_seen))
            .collect();
        expected.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
        let expected: Vec<usize> = expected.into_iter().map(|(key, _, _)| key).collect();
        assert_ne!(
            first_seen, expected,
            "fixture must distinguish first-seen order from hottest-first order",
        );

        let targets = polymorphic_closure_inline_targets(&exec.unit, func, idx)
            .expect("weighted polymorphic site should qualify for native PIC");
        assert_eq!(
            targets, expected,
            "profile-guided PIC target order should be hottest-first",
        );
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

    #[cfg(feature = "native-jit")]
    fn shaped_dispatch_program(targets: usize, calls: usize) -> String {
        let dispatch = match targets {
            2 => {
                r#"
        if i % 2 == 0 {
            total = total + dispatch(f: read a, x: read i)
        } else {
            total = total + dispatch(f: read b, x: read i)
        }
"#
            }
            3 => {
                r#"
        if i % 3 == 0 {
            total = total + dispatch(f: read a, x: read i)
        } else if i % 3 == 1 {
            total = total + dispatch(f: read b, x: read i)
        } else {
            total = total + dispatch(f: read c, x: read i)
        }
"#
            }
            _ => unreachable!("shape fixture supports two or three targets"),
        };
        format!(
            r#"
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {{
    let mut j = 0
    let mut total = 0
    while j < 4 {{
        total = total + f(x + j)
        j = j + 1
    }}
    return total
}}

fn main() -> Unit {{
    let a: Fn(Int) -> Int = |x| {{ return x * 2 }}
    let b: Fn(Int) -> Int = |x| {{ return x + 7 }}
    let c: Fn(Int) -> Int = |x| {{ return 0 - x }}
    let mut i = 0
    let mut total = 0
    while i < {calls} {{
{dispatch}
        i = i + 1
    }}
    Output.write(message: read String.from_int(value: total))
    return Unit
}}
"#
        )
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_shape_key_uses_stable_abi_metadata_only() {
        let short = VmValue::List(Rc::new(RefCell::new(TypedVec::Ints(vec![1]))));
        let long = VmValue::List(Rc::new(RefCell::new(TypedVec::Ints(vec![1, 2, 3]))));
        assert_eq!(
            ShapeKey::from_values([&VmValue::Int(1), &short]),
            ShapeKey::from_values([&VmValue::Int(99), &long]),
            "scalar payloads and collection lengths must not enter the shape",
        );

        let closure_a = VmValue::Closure(Rc::new(VmClosure {
            function: 1,
            captures: Vec::new(),
        }));
        let closure_b = VmValue::Closure(Rc::new(VmClosure {
            function: 2,
            captures: Vec::new(),
        }));
        assert_eq!(
            ShapeKey::from_values([&closure_a]),
            ShapeKey::from_values([&closure_b]),
            "closure target identity belongs to the existing mono/PIC feedback, not whole-function shapes",
        );

        let left = VmValue::Struct(Rc::new(VmStruct::from_named(
            "LeftShape",
            [("value", VmValue::Int(1))],
        )));
        let right = VmValue::Struct(Rc::new(VmStruct::from_named(
            "RightShape",
            [("value", VmValue::Int(1))],
        )));
        assert_ne!(
            ShapeKey::from_values([&left]),
            ShapeKey::from_values([&right]),
            "interned concrete layouts must distinguish aggregate shapes",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_alternating_closures_reuse_the_pic_shape() {
        let source = shaped_dispatch_program(2, 140);
        let expected = compile(&source)
            .eval_main_with_args(Vec::<String>::new())
            .expect("interpreter run");
        let (actual, stats) = with_native_cost_model_disabled(|| {
            compile(&source).eval_main_with_args_native_with_stats(Vec::<String>::new())
        })
        .expect("native run");
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.value, expected.value);
        assert!(
            stats.profile_closure_pic_arms >= 2,
            "both closure targets should share the dispatcher's PIC-backed shape; stats={stats:?}",
        );
        assert!(stats.shape_cache_hits > 0, "stats={stats:?}");
        assert_eq!(stats.shape_limit_fallbacks, 0, "stats={stats:?}");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_third_closure_uses_pic_without_shape_fallback() {
        let source = shaped_dispatch_program(3, 180);
        let expected = compile(&source)
            .eval_main_with_args(Vec::<String>::new())
            .expect("interpreter run");
        let (actual, stats) = with_native_cost_model_disabled(|| {
            compile(&source).eval_main_with_args_native_with_stats(Vec::<String>::new())
        })
        .expect("native run");
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.value, expected.value);
        assert_eq!(stats.shape_limit_fallbacks, 0, "stats={stats:?}");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_multishape_forced_deopt_matches_interpreter() {
        let source = shaped_dispatch_program(3, 100);
        let expected = compile(&source)
            .eval_main_with_args(Vec::<String>::new())
            .expect("interpreter run");
        let actual = compile(&source)
            .eval_main_with_args_native_force_all_safepoints(Vec::<String>::new())
            .expect("forced-deopt native run");
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.value, expected.value);
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

    #[test]
    fn public_default_arms_resource_budgets() {
        let limits = VmLimits::default();
        assert!(limits.step_budget.is_some());
        assert!(limits.allocation_budget.is_some());
        assert!(limits.stdout_budget.is_some());
        assert!(limits.intrinsic_call_budget.is_some());
        assert!(limits.provider_call_budget.is_some());

        let trusted = VmLimits::unbounded_for_trusted_host();
        assert!(trusted.max_depth > limits.max_depth);
        assert!(trusted.step_budget.is_none());
        assert!(trusted.allocation_budget.is_none());
        assert!(trusted.stdout_budget.is_none());
        assert!(trusted.intrinsic_call_budget.is_none());
        assert!(trusted.provider_call_budget.is_none());
    }
}
