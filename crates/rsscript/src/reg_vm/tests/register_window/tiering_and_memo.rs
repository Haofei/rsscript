    #[cfg(feature = "native-jit")]
    fn native_constant_func(name: &str, value: i64) -> RegFunction {
        native_test_function(
            name,
            0,
            1,
            vec![
                RegInstr::LoadInt { dst: 0, value },
                RegInstr::Return { src: 0 },
            ],
        )
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_ladder_promotes_and_prefers_optimized_dispatch() {
        let mut vm = empty_vm();
        let mut native = NativeState::new_with_opt(1, false, true, false, true, false, false)
            .expect("native ladder");
        native.optimize_work_threshold = u64::MAX;
        vm.native = Some(native);
        vm.prepare_frame(0, 1).expect("frame");
        let func = native_constant_func("promote", 7);

        assert!(matches!(
            vm.try_native(&func, 0),
            NativeAttempt::Completed(VmValue::Int(7))
        ));
        {
            let native = vm.native.as_ref().expect("native");
            assert_eq!(native.stats.baseline_compiles, 1);
            assert_eq!(native.stats.baseline_calls, 1);
            assert_eq!(native.stats.optimized_compiles, 0);
        }

        vm.native.as_mut().expect("native").optimize_work_threshold = 0;
        for _ in 0..2 {
            assert!(matches!(
                vm.try_native(&func, 0),
                NativeAttempt::Completed(VmValue::Int(7))
            ));
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.baseline_compiles, 1);
        assert_eq!(stats.optimized_compiles, 1);
        assert_eq!(stats.promotions, 1);
        assert_eq!(stats.baseline_calls, 1);
        assert_eq!(
            stats.optimized_calls, 2,
            "the promoted cache must remain the preferred dispatch"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_ladder_does_not_promote_below_work_threshold() {
        let mut vm = empty_vm();
        let mut native = NativeState::new_with_opt(1, false, true, false, true, false, false)
            .expect("native ladder");
        native.optimize_work_threshold = u64::MAX;
        vm.native = Some(native);
        vm.prepare_frame(0, 1).expect("frame");
        let func = native_constant_func("stay_baseline", 9);

        for _ in 0..3 {
            assert!(matches!(
                vm.try_native(&func, 0),
                NativeAttempt::Completed(VmValue::Int(9))
            ));
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.baseline_compiles, 1);
        assert_eq!(stats.baseline_calls, 3);
        assert_eq!(stats.optimized_compiles, 0);
        assert_eq!(stats.optimized_calls, 0);
        assert_eq!(stats.promotions, 0);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_ladder_shares_admission_budget_across_modules() {
        let mut vm = empty_vm();
        let mut native = NativeState::new_with_opt(1, false, true, false, true, false, false)
            .expect("native ladder");
        native.optimize_work_threshold = u64::MAX;
        vm.native = Some(native);
        vm.prepare_frame(0, 1).expect("frame");
        let func = native_constant_func("shared_budget", 11);

        assert!(matches!(
            vm.try_native(&func, 0),
            NativeAttempt::Completed(VmValue::Int(11))
        ));
        let baseline_bytes = vm
            .native
            .as_ref()
            .expect("native")
            .admission
            .admitted_code_bytes;
        {
            let native = vm.native.as_mut().expect("native");
            native.admission.max_code_bytes = baseline_bytes;
            native.optimize_work_threshold = 0;
        }
        assert!(matches!(
            vm.try_native(&func, 0),
            NativeAttempt::Completed(VmValue::Int(11))
        ));
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.baseline_calls, 2);
        assert_eq!(stats.optimized_calls, 0);
        assert_eq!(stats.optimized_compiles, 0);
        assert_eq!(stats.promotions, 0);
        assert_eq!(stats.admission_rejected, 1);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn baseline_only_mode_preserves_precise_deopt() {
        let mut vm = empty_vm();
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, true, true, false, false)
                .expect("baseline-only native module"),
        );
        let func = Rc::new(add_then_square_func());
        let big = 4_000_000_000i64;
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

        assert!(matches!(vm.try_native(&func, 0), NativeAttempt::Resumed));
        let native = vm.native.as_ref().expect("native");
        assert!(native.optimized_module.is_none());
        assert_eq!(native.stats.baseline_compiles, 1);
        assert_eq!(native.stats.optimized_compiles, 0);
        assert_eq!(native.stats.promotions, 0);
        assert_eq!(vm.frames.last().expect("frame").ip, 2);
        assert_eq!(*vm.reg(1), VmValue::Int(big + 1));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_ladder_promotes_region() {
        let source = "\
fn hot(limit: Int) -> Int {
    Log.write(message: \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        total = total + i * 3 + 1
        i = i + 1
    }
    Log.write(message: \"end\")
    return total
}

fn main() -> Unit {
    Log.write(message: String.from_int(value: hot(limit: 8)))
    return Unit
}
";
        let executable = reg_vm_compile_source("osr-ladder.rss", source).expect("source compiles");
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::new(),
            HashMap::<String, ExternalFunction>::new(),
        );
        let mut native = NativeState::new_with_opt(1, false, true, false, true, true, false)
            .expect("native ladder");
        native.optimize_work_threshold = 0;
        vm.native = Some(native);
        vm.jit_enabled = true;
        vm.jit_force_all = true;

        vm.run_program("main").expect("program runs");
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.baseline_compiles, 1);
        assert_eq!(stats.optimized_compiles, 1);
        assert_eq!(stats.promotions, 1);
        assert_eq!(stats.baseline_calls, 0);
        assert_eq!(stats.optimized_calls, 1);
        assert_eq!(stats.osr_entries, 1);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_admission_budget_bounds_many_functions_and_keeps_existing_dispatch() {
        let mut vm = empty_vm();
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, 1).expect("frame");
        let functions: Vec<_> = (0..32)
            .map(|index| native_constant_func(&format!("constant_{index}"), index))
            .collect();

        assert!(matches!(
            vm.try_native(&functions[0], 0),
            NativeAttempt::Completed(VmValue::Int(0))
        ));
        let first_bytes = vm
            .native
            .as_ref()
            .expect("native")
            .admission
            .admitted_code_bytes;
        assert!(first_bytes > 0);
        vm.native.as_mut().expect("native").admission.max_code_bytes = first_bytes;

        for (index, func) in functions.iter().enumerate().skip(1) {
            assert!(
                matches!(vm.try_native(func, 0), NativeAttempt::Fallback),
                "function {index} should fall back after admission is exhausted",
            );
        }
        assert!(
            matches!(
                vm.try_native(&functions[0], 0),
                NativeAttempt::Completed(VmValue::Int(0))
            ),
            "an entry admitted before exhaustion must remain dispatchable",
        );

        let native = vm.native.as_ref().expect("native");
        assert_eq!(native.stats.compiled, 1);
        assert_eq!(native.stats.admission_admitted, 1);
        assert_eq!(native.stats.admission_admitted_bytes, first_bytes);
        assert_eq!(native.stats.admission_rejected, 31);
        assert_eq!(native.stats.admission_rejected_bytes, 0);
        assert_eq!(native.admission.admitted_code_bytes, first_bytes);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_post_compile_budget_rejection_falls_back_without_admission() {
        let mut vm = empty_vm();
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.native.as_mut().expect("native").admission.max_code_bytes = 1;
        vm.prepare_frame(0, 1).expect("frame");
        let func = native_constant_func("too_large", 7);

        assert!(matches!(vm.try_native(&func, 0), NativeAttempt::Fallback));
        let second = native_constant_func("blocked_after_oversize", 8);
        assert!(matches!(vm.try_native(&second, 0), NativeAttempt::Fallback));
        let native = vm.native.as_ref().expect("native");
        assert_eq!(native.stats.compiled, 0);
        assert_eq!(native.stats.admission_admitted, 0);
        assert_eq!(native.stats.admission_admitted_bytes, 0);
        assert_eq!(native.stats.admission_rejected, 2);
        assert!(
            native.stats.admission_rejected_bytes > 1,
            "post-compile rejection should report the emitted bytes: {:?}",
            native.stats,
        );
        assert_eq!(native.admission.admitted_code_bytes, 0);
        assert!(native.admission.code_exhausted);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_zero_compile_time_budget_rejects_before_compilation() {
        let mut vm = empty_vm();
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.native
            .as_mut()
            .expect("native")
            .admission
            .max_compile_nanos = 0;
        vm.prepare_frame(0, 1).expect("frame");
        let func = native_constant_func("no_compile_time", 9);

        assert!(matches!(vm.try_native(&func, 0), NativeAttempt::Fallback));
        let native = vm.native.as_ref().expect("native");
        assert_eq!(native.stats.compiled, 0);
        assert_eq!(native.stats.compile_nanos, 0);
        assert_eq!(native.stats.admission_admitted, 0);
        assert_eq!(native.stats.admission_rejected, 1);
        assert_eq!(native.stats.admission_rejected_bytes, 0);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_memoizes_read_only_collection_metadata_once() {
        let source = r#"

fn hot(
    table: read Map<Int, Int>,
    set: read Set<Int>,
    sorted: read SortedSet<Int>,
    sorted_table: read SortedMap<Int, Int>,
    queue: read Deque<Int>,
    limit: Int
) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + Map.len<Int, Int>(map: read table)
        total = total + Set.len<Int>(set: read set)
        total = total + SortedSet.len<Int>(set: read sorted)
        total = total + SortedMap.len<Int, Int>(map: read sorted_table)
        total = total + Deque.len<Int>(deque: read queue)
        if Map.is_empty<Int, Int>(map: read table) {
            total = total + 100
        }
        if Set.is_empty<Int>(set: read set) {
            total = total + 100
        }
        if SortedSet.is_empty<Int>(set: read sorted) {
            total = total + 100
        }
        if SortedMap.is_empty<Int, Int>(map: read sorted_table) {
            total = total + 100
        }
        if Deque.is_empty<Int>(deque: read queue) {
            total = total + 100
        }
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    local table = Map<Int, Int>.new()
    local set = Set.new<Int>()
    local sorted = SortedSet.new<Int>()
    local sorted_table = SortedMap<Int, Int>.new()
    local queue = Deque<Int>.new()
    Map.insert<Int, Int>(map: mut table, key: read 1, value: read 1)
    Set.insert(set: mut set, value: read 1)
    let _sorted_inserted = SortedSet.insert<Int>(set: mut sorted, value: read 1)
    SortedMap.insert<Int, Int>(map: mut sorted_table, key: read 1, value: read 1)
    Deque.push_back<Int>(deque: mut queue, value: read 1)
    let total = hot(
        table: read table,
        set: read set,
        sorted: read sorted,
        sorted_table: read sorted_table,
        queue: read queue,
        limit: 50
    )
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("collection-metadata.rss", source).expect("lowering should work");
        let hot = executable.unit.function_ids["hot"];
        let (jit, _, _, _, _) =
            translate_to_native_jit(&executable.unit, executable.unit.functions[hot].as_ref())
                .expect("read-only collection metadata loop should translate");

        for helper in [
            vm_jit::HostHelper::MapLen,
            vm_jit::HostHelper::SetLen,
            vm_jit::HostHelper::ListLen,
            vm_jit::HostHelper::SortedMapLen,
            vm_jit::HostHelper::DequeLen,
            vm_jit::HostHelper::MapIsEmpty,
            vm_jit::HostHelper::SetIsEmpty,
            vm_jit::HostHelper::SortedSetIsEmpty,
            vm_jit::HostHelper::SortedMapIsEmpty,
            vm_jit::HostHelper::DequeIsEmpty,
        ] {
            assert!(
                jit.code.iter().any(|instr| matches!(
                    instr,
                    vm_jit::JitInstr::MemoizedHostCall { helper: actual, .. }
                        if *actual == helper
                )),
                "{helper:?} should be lazily memoized; code={:#?}",
                jit.code
            );
        }

        reset_jit_collection_metadata_helper_calls();
        let (output, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert_eq!(output.stdout.trim(), "250");
        assert!(stats.native_calls > 0, "hot function must run natively");
        assert_eq!(
            jit_collection_metadata_helper_calls(),
            10,
            "each metadata query site should call its helper once per native invocation"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_nested_loop_memo_resets_once_per_outer_activation() {
        let source = r#"

fn hot(queue: mut Deque<Int>, outer_limit: Int, inner_limit: Int) -> Int {
    let mut outer = 0
    let mut total = 0
    while outer < outer_limit {
        Deque.push_back<Int>(deque: mut queue, value: read outer)
        let mut inner = 0
        while inner < inner_limit {
            total = total + Deque.len<Int>(deque: read queue)
            inner = inner + 1
        }
        outer = outer + 1
    }
    return total
}

fn main() -> Unit {
    local queue = Deque.new<Int>()
    let total = hot(queue: mut queue, outer_limit: 3, inner_limit: 4)
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("nested-loop-memo.rss", source).expect("lowering should work");
        let hot = executable.unit.function_ids["hot"];
        let (jit, _, _, _, _) =
            translate_to_native_jit(&executable.unit, executable.unit.functions[hot].as_ref())
                .expect("structured nested loop should translate");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::DequeLen,
                    ..
                }
            )),
            "inner-loop Deque.len should be memoized: {:#?}",
            jit.code
        );
        assert_eq!(jit.memo_scopes.len(), 1, "{:#?}", jit.memo_scopes);

        reset_jit_collection_metadata_helper_calls();
        let (output, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert_eq!(output.stdout.trim(), "24");
        assert!(stats.native_calls > 0, "hot function must run natively");
        assert_eq!(
            jit_collection_metadata_helper_calls(),
            3,
            "Deque.len should run once per outer-loop activation"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn collection_len_memoization_respects_projection_writes() {
        let list_set = vec![vm_jit::JitInstr::HostCall {
            helper: vm_jit::HostHelper::ListSetInt,
            dst: 3,
            args: vec![
                vm_jit::HostArg::Reg(0),
                vm_jit::HostArg::Reg(1),
                vm_jit::HostArg::Reg(2),
            ],
        }];
        assert!(
            crate::reg_vm::native_loop_preserves_heap_projection(
                &list_set,
                0,
                list_set.len(),
                vm_jit::HostHeapProjection::CollectionLen,
            ),
            "element replacement does not change collection length"
        );

        let map_insert = vec![vm_jit::JitInstr::HostCall {
            helper: vm_jit::HostHelper::MapInsertInt,
            dst: 3,
            args: vec![
                vm_jit::HostArg::Reg(0),
                vm_jit::HostArg::Reg(1),
                vm_jit::HostArg::Reg(2),
            ],
        }];
        assert!(
            !crate::reg_vm::native_loop_preserves_heap_projection(
                &map_insert,
                0,
                map_insert.len(),
                vm_jit::HostHeapProjection::CollectionLen,
            ),
            "insertion can change collection length"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_memoizes_length_across_unrelated_fresh_collection_write() {
        let source = r#"

fn hot(values: read List<Int>, limit: Int) -> Int {
    local scratch = List.new<Int>()
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + List.len<Int>(list: read values)
        List.push<Int>(list: mut scratch, value: read index)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("fresh-collection-alias.rss", source).expect("lowering");
        let hot = executable.unit.function_ids["hot"];
        let (jit, _, _, _, _) =
            translate_to_native_jit(&executable.unit, executable.unit.functions[hot].as_ref())
                .expect("fresh collection mutation loop should translate");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::ListLen,
                    ..
                }
            )),
            "a write to a distinct fresh collection cannot invalidate values.len: {:#?}",
            jit.code
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_external_collection_receivers_may_alias() {
        let source = r#"

fn hot(values: mut List<Int>, other: mut List<Int>, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + List.len<Int>(list: read values)
        List.push<Int>(list: mut other, value: read index)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("external-collection-alias.rss", source).expect("lowering");
        let hot = executable.unit.function_ids["hot"];
        let (jit, _, _, _, _) =
            translate_to_native_jit(&executable.unit, executable.unit.functions[hot].as_ref())
                .expect("external collection mutation loop should translate");

        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::ListLen,
                    ..
                }
            )),
            "external ABI handles may alias even when they occupy different registers: {:#?}",
            jit.code
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_list_set_preserves_len_but_push_invalidates_it() {
        let translate = |name: &str, mutation: &str| {
            let source = format!(
                r#"

fn hot(values: mut List<Int>, limit: Int) -> Int {{
    let mut index = 0
    let mut total = 0
    while index < limit {{
        total = total + List.len<Int>(list: read values)
        {mutation}
        index = index + 1
    }}
    return total
}}

fn main() -> Unit {{
    return Unit
}}
"#
            );
            let executable = reg_vm_compile_source(name, &source).expect("lowering");
            let hot = executable.unit.function_ids["hot"];
            translate_to_native_jit(&executable.unit, executable.unit.functions[hot].as_ref())
                .expect("collection mutation loop should translate")
                .0
        };

        let set = translate(
            "list-set-preserves-len.rss",
            "List.set<Int>(list: mut values, index: 0, value: read index)",
        );
        assert!(
            set.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::ListLen,
                    ..
                } | vm_jit::JitInstr::ListLenDirect { .. }
            )),
            "List.set writes elements but preserves a memoized or direct Len: {:#?}",
            set.code
        );

        let push = translate(
            "list-push-invalidates-len.rss",
            "List.push<Int>(list: mut values, value: read index)",
        );
        assert!(
            !push.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::ListLen,
                    ..
                }
            )),
            "List.push can change Len: {:#?}",
            push.code
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn field_slot_write_on_distinct_fresh_receiver_preserves_query() {
        let code = vec![
            RegInstr::CallTypedIntrinsic {
                dst: 0,
                intrinsic: RegIntrinsic::ListNew,
                type_arg: "Int".to_string(),
                args: Vec::new(),
            },
            RegInstr::CallTypedIntrinsic {
                dst: 1,
                intrinsic: RegIntrinsic::ListNew,
                type_arg: "Int".to_string(),
                args: Vec::new(),
            },
            RegInstr::GetFieldSlot {
                dst: 2,
                base: 0,
                slot: 0,
            },
            RegInstr::SetFieldSlot {
                dst: 3,
                base: 1,
                slot: 0,
                value: 2,
            },
            RegInstr::Jump { target: 2 },
        ];
        let query_args = vec![vm_jit::HostArg::Reg(0), vm_jit::HostArg::ImmI64(0)];
        let jit_code = vec![
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::ListNewInt,
                dst: 0,
                args: Vec::new(),
            },
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::ListNewInt,
                dst: 1,
                args: Vec::new(),
            },
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::FieldInt,
                dst: 2,
                args: query_args.clone(),
            },
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::FieldSetInt,
                dst: 1,
                args: vec![
                    vm_jit::HostArg::Reg(1),
                    vm_jit::HostArg::ImmI64(0),
                    vm_jit::HostArg::Reg(2),
                ],
            },
            vm_jit::JitInstr::Jump { target: 2 },
        ];

        assert!(
            crate::reg_vm::native_loop_preserves_field_slot_for_receiver(
                &code,
                &jit_code,
                &[
                    NativeTy::Handle,
                    NativeTy::Handle,
                    NativeTy::Int,
                    NativeTy::Int,
                ],
                0,
                &query_args,
                2,
                5,
                2,
            ),
            "same field slot on distinct proven-fresh receivers cannot alias"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_does_not_memoize_length_across_insert() {
        let source = r#"

fn hot(table: mut Map<Int, Int>, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + Map.len<Int, Int>(map: read table)
        Map.insert<Int, Int>(map: mut table, key: read index, value: read index)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("collection-metadata-write.rss", source).expect("lowering");
        let hot = executable.unit.function_ids["hot"];
        let (jit, _, _, _, _) =
            translate_to_native_jit(&executable.unit, executable.unit.functions[hot].as_ref())
                .expect("collection mutation loop should translate");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::MapLen,
                    ..
                }
            )),
            "Map.len must remain a real call when insert can change length: {:#?}",
            jit.code
        );
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::MapLen,
                    ..
                }
            )),
            "Map.len cannot be cached across insert"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_does_not_memoize_variant_loop_helper_args() {
        let source = r#"
fn hot(prefix: read String, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        let line = String.from_int(value: index)
        if String.starts_with(value: read line, prefix: read prefix) {
            total = total + 1
        }
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    let total = hot(prefix: read "1", limit: 10)
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[hot].as_ref())
            .expect("variant string helper loop should still translate");

        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::StringStartsWith,
                    ..
                }
            )),
            "StringStartsWith should not be memoized when an argument changes each iteration; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_does_not_memoize_field_of_changing_root() {
        let source = r#"
struct Box {
    value: Int
}

fn hot(first: read Box, second: read Box, limit: Int) -> Int {
    let mut current = first
    let mut index = 0
    let mut total = 0
    while index < limit {
        if index % 2 == 0 {
            current = first
        } else {
            current = second
        }
        total = total + current.value
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    return Unit
}
"#;
        let mut program = parse_source("changing-field-root.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[hot].as_ref())
            .expect("changing-root field loop should still translate");

        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::FieldInt,
                    ..
                }
            )),
            "a field load whose base changes roots must not be memoized: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_call_invalidates_field_memoization() {
        let mut module = vm_jit::NativeModule::new(jit_host_helpers()).expect("native module");
        let callee = module
            .compile(&vm_jit::JitFunction {
                n_params: 0,
                n_regs: 1,
                reg_types: vec![vm_jit::JitValueType::Int],
                zero_init_regs: Vec::new(),
                code: vec![
                    vm_jit::JitInstr::LoadInt { dst: 0, value: 0 },
                    vm_jit::JitInstr::Return { src: 0 },
                ],
                memo_scopes: Vec::new(),
                cold_blocks: Vec::new(),
            })
            .expect("compile test callee");
        let args = vec![vm_jit::HostArg::Reg(0), vm_jit::HostArg::ImmI64(0)];
        let code = vec![
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::FieldInt,
                dst: 1,
                args: args.clone(),
            },
            vm_jit::JitInstr::CallNative {
                callee,
                dst: 2,
                args: Vec::new(),
            },
        ];

        assert!(
            !crate::reg_vm::native_field_load_slot_not_stored_in_loop(&args, &code, 0, code.len()),
            "an unsummarized native call must kill field-load memoization"
        );
        let reg_code = vec![
            RegInstr::GetFieldSlot {
                dst: 1,
                base: 0,
                slot: 0,
            },
            RegInstr::CallExternal {
                dst: 2,
                key: "unknown".to_string(),
                args: Vec::new(),
                mut_args: Vec::new(),
            },
        ];
        assert!(
            !crate::reg_vm::native_loop_preserves_field_slot_for_receiver(
                &reg_code,
                &code,
                &[NativeTy::Handle, NativeTy::Int, NativeTy::Int],
                1,
                &args,
                0,
                code.len(),
                0,
            ),
            "the receiver-aware query must also treat an unsummarized call as a universal write"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_scopes_nested_loop_memoization_to_each_activation() {
        let source = r#"
fn hot(outer_limit: Int, inner_limit: Int) -> Int {
    let mut outer = 0
    let mut total = 0
    while outer < outer_limit {
        let line = String.from_int(value: outer)
        let mut inner = 0
        while inner < inner_limit {
            total = total + String.len(value: read line)
            inner = inner + 1
        }
        outer = outer + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: String.from_int(value: hot(outer_limit: 20, inner_limit: 2)))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[hot].as_ref())
            .expect("nested loop should translate");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::StringLen,
                    memo_slot: 0,
                    ..
                }
            )),
            "the invariant helper should be memoized inside the inner loop: {:#?}",
            jit.code,
        );
        assert!(
            jit.memo_scopes
                .iter()
                .any(|scope| scope.memo_slots.as_slice() == [0]),
            "the memo slot must be reset on each dynamic inner-loop activation: {:#?}",
            jit.memo_scopes,
        );
    }
