    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_inlines_mailbox_send_and_take_in_hot_loop() {
        let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/vm-jit/kernels/mailbox_ring_only.rss");
        let source =
            std::fs::read_to_string(source_path).expect("mailbox benchmark source should exist");
        let executable = reg_vm_compile_source("mailbox_ring_only.rss", &source)
            .expect("lowering should succeed");
        let hot = executable.unit.function_ids["hot"];
        let hot_func = executable.unit.functions[hot].as_ref();
        let candidate = detect_natural_loops(&hot_func.code)
            .into_iter()
            .find(|lp| {
                hot_func.code.get(lp.header..lp.exit).is_some_and(|region| {
                    region
                        .iter()
                        .any(|instr| matches!(instr, RegInstr::CallKnown { .. }))
                })
            })
            .expect("hot loop should contain calls before inlining");
        let (code, n_regs, _ip_map) = native_inline_leaf_calls(
            &executable.unit,
            hot_func,
            true,
            Some((candidate.header, candidate.exit)),
        )
        .unwrap_or_else(|| {
            panic!(
                "hot loop calls should be J3-inlinable; region={:#?}",
                &hot_func.code[candidate.header..candidate.exit],
            )
        });
        let inlined_loop = detect_natural_loops(&code)
            .into_iter()
            .find(|lp| {
                code.get(lp.header..lp.exit).is_some_and(|region| {
                    region
                        .iter()
                        .any(|instr| matches!(instr, RegInstr::MatchOption { .. }))
                })
            })
            .expect("inlined hot loop should still be detectable and contain Option match");
        assert!(
            !code[inlined_loop.header..inlined_loop.exit]
                .iter()
                .any(|instr| matches!(instr, RegInstr::CallKnown { .. })),
            "hot loop should not contain CallKnown after J3 inlining; region={:#?}",
            &code[inlined_loop.header..inlined_loop.exit],
        );
        let (code, n_regs, _) = native_lower_checked_payload_intrinsics_in_region(
            &code,
            n_regs,
            inlined_loop.header,
            inlined_loop.exit,
        )
        .expect("checked payload pass should accept inlined hot loop");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after checked payload pass");
        let (code, n_regs, _, _) =
            native_scalar_replace_results_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("result SR should accept inlined hot loop");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after result SR");
        let (code, n_regs, _, _) =
            native_scalar_replace_options_in_region(&code, n_regs, lp.header, lp.exit)
                .unwrap_or_else(|| {
                    panic!(
                        "option SR should accept inlined hot loop; region={:#?}",
                        &code[lp.header..lp.exit],
                    )
                });
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after option SR");
        let (code, n_regs, _, _) =
            native_scalar_replace_variants_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("variant SR should accept inlined hot loop");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after variant SR");
        let (code, n_regs, _, _) =
            native_scalar_replace_structs_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("struct SR should accept inlined hot loop");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after struct SR");
        let (code, n_regs, _) =
            native_loop_carried_struct_in_region(&code, n_regs, lp.header, lp.exit)
                .unwrap_or_else(|| (code.clone(), n_regs, (0..code.len()).collect()));
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after loop-carried struct pass");
        let (jit, _, derived_liveins, scalar_fields, _, _, _) = translate_osr_loop(
            &code,
            n_regs,
            hot_func.params,
            hot_func.captures,
            lp,
        )
        .unwrap_or_else(|| {
            panic!(
                "fully transformed mailbox hot loop should lower to OSR native IR; region={:#?}",
                &code[lp.header..lp.exit],
            )
        });
        let memoized_field_slot = |helper, slot| {
            jit.code.iter().any(|instr| {
                matches!(
                    instr,
                    vm_jit::JitInstr::MemoizedHostCall {
                        helper: h,
                        args,
                        ..
                    } if *h == helper
                        && matches!(args.get(1), Some(vm_jit::HostArg::ImmI64(s)) if *s == slot)
                )
            })
        };
        let host_helper = |helper| {
            jit.code.iter().any(|instr| {
                matches!(
                    instr,
                    vm_jit::JitInstr::HostCall { helper: h, .. }
                        | vm_jit::JitInstr::MemoizedHostCall { helper: h, .. }
                        if *h == helper
                )
            })
        };
        let scalar_slot = |slot, writeback| {
            scalar_fields
                .iter()
                .any(|field| field.field_slot == slot && field.writeback == writeback)
        };
        assert!(
            scalar_slot(0, false) && scalar_slot(1, true) && scalar_slot(2, true),
            "capacity/head/count fields should be scalar OSR fields; scalar={:#?}; jit code={:#?}",
            scalar_fields,
            jit.code,
        );
        assert!(
            derived_liveins
                .iter()
                .filter(|livein| livein.field_slot == 3)
                .count()
                >= 3,
            "values-list field aliases should be derived OSR live-ins; derived={:#?}; jit code={:#?}",
            derived_liveins,
            jit.code,
        );
        assert!(
            !host_helper(vm_jit::HostHelper::ListLen)
                && !host_helper(vm_jit::HostHelper::ListSetInt)
                && !host_helper(vm_jit::HostHelper::ListGetInt),
            "values-list len/set/get operations should avoid per-iteration list helpers; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListLenDirect { .. }))
                && jit
                    .code
                    .iter()
                    .any(|instr| matches!(instr, vm_jit::JitInstr::ListSetIntDirect { .. }))
                && jit
                    .code
                    .iter()
                    .any(|instr| matches!(instr, vm_jit::JitInstr::ListGetIntDirect { .. })),
            "values-list field should lower to direct len/set/get operations; jit code: {:#?}",
            jit.code,
        );
        assert!(
            !memoized_field_slot(vm_jit::HostHelper::FieldInt, 1)
                && !memoized_field_slot(vm_jit::HostHelper::FieldInt, 2),
            "mutated head/count fields must not be memoized; jit code: {:#?}",
            jit.code,
        );
        assert!(
            !host_helper(vm_jit::HostHelper::FieldInt)
                && !host_helper(vm_jit::HostHelper::FieldSetInt),
            "scalar mailbox fields should avoid per-iteration field helpers; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_scalarizes_struct_field_rw_loop() {
        let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/micro/struct_field_rw.rss");
        let source =
            std::fs::read_to_string(source_path).expect("struct benchmark source should exist");
        let executable =
            reg_vm_compile_source("struct_field_rw.rss", &source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let lp = crate::reg_vm::tier::select_osr_candidate_loop(&executable.unit, func)
            .or_else(|| detect_natural_loops(&func.code).into_iter().next())
            .expect("struct benchmark hot loop should be selected");
        let (code, n_regs, _) =
            native_inline_leaf_calls(&executable.unit, func, true, Some((lp.header, lp.exit)))
                .expect("struct benchmark loop should inline");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after inlining");
        let (code, n_regs, _, _) =
            native_scalar_replace_structs_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("loop-local struct pass should accept or return identity");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after struct pass");
        let (code, n_regs, _) = native_loop_carried_struct_in_region(
            &code, n_regs, lp.header, lp.exit,
        )
        .unwrap_or_else(|| {
            panic!(
                "loop-carried struct pass should scalarize struct_field_rw; region={:#?}",
                &code[lp.header..lp.exit],
            )
        });
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after loop-carried struct pass");
        assert!(
            !code[lp.header..lp.exit].iter().any(|instr| matches!(
                instr,
                RegInstr::GetFieldSlot { .. } | RegInstr::SetFieldSlot { .. }
            )),
            "scalarized struct loop should not retain field helpers; region={:#?}",
            &code[lp.header..lp.exit],
        );
        translate_osr_loop(&code, n_regs, func.params, func.captures, lp).unwrap_or_else(|| {
            panic!(
                "scalarized struct_field_rw loop should translate to OSR native IR; region={:#?}",
                &code[lp.header..lp.exit],
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_profiles_stored_polymorphic_closure_call() {
        let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/vm-jit/kernels/dynamic_closure_call.rss");
        let source = std::fs::read_to_string(source_path)
            .expect("dynamic closure benchmark source should exist");
        let executable = reg_vm_compile_source("dynamic_closure_call.rss", &source)
            .expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let main_func = executable.unit.functions[main].as_ref();
        let call_idx = main_func
            .code
            .iter()
            .position(|instr| matches!(instr, RegInstr::CallClosure { .. }))
            .expect("main loop should contain a stored closure call");
        let candidate = detect_natural_loops(&main_func.code)
            .into_iter()
            .find(|lp| {
                main_func
                    .code
                    .get(lp.header..lp.exit)
                    .is_some_and(|region| {
                        region
                            .iter()
                            .any(|instr| matches!(instr, RegInstr::CallClosure { .. }))
                    })
            })
            .expect("dynamic closure loop should be detected");
        assert_eq!(
            super::super::tier::select_osr_candidate_loop(&executable.unit, main_func)
                .map(|lp| lp.header),
            Some(candidate.header),
            "cold auto OSR selection should arm the transformable closure loop"
        );

        executable
            .eval_main_with_args(vec!["400".to_string()])
            .expect("benchmark should run and warm profile");

        {
            let profile = main_func.profile.borrow();
            let feedback = profile
                .as_ref()
                .and_then(|profile| profile.call_sites.get(&call_idx))
                .unwrap_or_else(|| panic!("profile should contain feedback for call {call_idx}"));
            assert_eq!(feedback.state(), MonoState::Polymorphic);
            assert_eq!(feedback.observed.len(), 2);
            assert!(feedback.captures_all_scalar);
        }
        assert!(
            polymorphic_closure_inline_targets(&executable.unit, main_func, call_idx).is_some(),
            "stored two-target closure call should qualify for J2 polymorphic inline"
        );

        assert_eq!(
            super::super::tier::select_osr_candidate_loop(&executable.unit, main_func)
                .map(|lp| lp.header),
            Some(candidate.header),
            "auto OSR selection should arm the transformable closure loop"
        );
        let (code, n_regs, _) = native_inline_leaf_calls(
            &executable.unit,
            main_func,
            true,
            Some((candidate.header, candidate.exit)),
        )
        .unwrap_or_else(|| {
            panic!(
                "stored polymorphic closure call should inline; region={:#?}",
                &main_func.code[candidate.header..candidate.exit],
            )
        });
        let (code, n_regs, _) = native_fuse_field_closure_metadata_reads(&code, n_regs)
            .expect("stored closure helper fusion should run");
        assert!(
            code.iter()
                .any(|instr| matches!(instr, RegInstr::NativeFieldClosureId { .. })),
            "stored closure field metadata should fuse away the intermediate FieldHandle; code={code:#?}",
        );
        let lp = detect_natural_loops(&code)
            .into_iter()
            .find(|lp| {
                code.get(lp.header..lp.exit).is_some_and(|region| {
                    region
                        .iter()
                        .any(|instr| matches!(instr, RegInstr::NativeFieldClosureId { .. }))
                })
            })
            .expect("inlined closure loop should remain detectable");
        translate_osr_loop(&code, n_regs, main_func.params, main_func.captures, lp).unwrap_or_else(
            || {
                panic!(
                    "polymorphic closure-dispatch loop should lower to OSR native IR; region={:#?}",
                    &code[lp.header..lp.exit],
                )
            },
        );

        main_func.osr_state.set(OsrTrigger::Unknown);
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            std::iter::empty::<(String, ExternalFunction)>().collect(),
        );
        assert_eq!(
            vm.resolve_osr_candidate(main_func),
            Some(candidate.header),
            "native VM resolver should arm the stored closure loop"
        );
        main_func.osr_state.set(OsrTrigger::Unknown);
        let (_out, stats) = executable
            .eval_main_with_args_native_with_stats(["2000"])
            .expect("adaptive native run should succeed");
        assert!(
            stats.osr_entries > 0,
            "warmed stored polymorphic closure loop should OSR-enter; stats={stats:?}; osr_state={:?}; call_count={}",
            main_func.osr_state.get(),
            main_func.call_count.get(),
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_fuses_field_closure_metadata_reads_when_handle_is_dead() {
        let code = vec![
            RegInstr::GetFieldSlot {
                dst: 2,
                base: 1,
                slot: 0,
            },
            RegInstr::NativeClosureId { dst: 3, closure: 2 },
            RegInstr::NativeClosureCapture {
                dst: 4,
                closure: 2,
                index: 0,
            },
            RegInstr::Return { src: 3 },
        ];

        let (fused, n_regs, ip_map) =
            native_fuse_field_closure_metadata_reads(&code, 5).expect("pass should run");
        assert_eq!(n_regs, 5);
        assert_eq!(ip_map, vec![0, 1, 2, 3]);
        assert!(matches!(fused[0], RegInstr::Move { dst: 2, src: 1 }));
        assert!(matches!(
            fused[1],
            RegInstr::NativeFieldClosureId {
                dst: 3,
                base: 1,
                slot: 0
            }
        ));
        assert!(matches!(
            fused[2],
            RegInstr::NativeFieldClosureCapture {
                dst: 4,
                base: 1,
                slot: 0,
                index: 0
            }
        ));

        let escaping = vec![
            RegInstr::GetFieldSlot {
                dst: 2,
                base: 1,
                slot: 0,
            },
            RegInstr::NativeClosureId { dst: 3, closure: 2 },
            RegInstr::Return { src: 2 },
        ];
        let (not_fused, _, _) =
            native_fuse_field_closure_metadata_reads(&escaping, 4).expect("pass should run");
        assert!(matches!(not_fused[0], RegInstr::GetFieldSlot { .. }));
        assert!(matches!(not_fused[1], RegInstr::NativeClosureId { .. }));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_selects_selfhost_mailbox_hot_loop() {
        let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/micro/selfhost_mailbox_bench.rss");
        let source = std::fs::read_to_string(source_path)
            .expect("selfhost mailbox benchmark source should exist");
        let executable = reg_vm_compile_source("selfhost_mailbox_bench.rss", &source)
            .expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let main_func = executable.unit.functions[main].as_ref();
        let loops = detect_natural_loops(&main_func.code);
        let selected = super::super::tier::select_osr_candidate_loop(&executable.unit, main_func);
        let hot_loop = loops
            .iter()
            .copied()
            .find(|lp| {
                let region = &main_func.code[lp.header..lp.exit];
                region
                    .iter()
                    .filter(|instr| matches!(instr, RegInstr::CallKnown { .. }))
                    .count()
                    >= 3
            })
            .expect("main cycles loop should be detected");
        assert_eq!(
            selected.map(|lp| lp.header),
            Some(hot_loop.header),
            "OSR selection should prefer the large hot mailbox loop over setup/drain loops",
        );
        let (code, n_regs, ip_map) = native_inline_leaf_calls(
            &executable.unit,
            main_func,
            true,
            Some((hot_loop.header, hot_loop.exit)),
        )
        .unwrap_or_else(|| {
            panic!(
                "selfhost mailbox hot loop calls should inline; region={:#?}",
                &main_func.code[hot_loop.header..hot_loop.exit],
            )
        });
        let header = ip_map
            .iter()
            .position(|&old| old == hot_loop.header)
            .expect("inlined header should map from original hot loop");
        let lp = detect_natural_loop_at(&code, header).unwrap_or_else(|| {
            panic!(
                "inlined selfhost mailbox hot loop should remain detectable; header={header}; code={code:#?}"
            )
        });
        let (code, n_regs, _) =
            native_lower_checked_payload_intrinsics_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("checked payload pass should accept selfhost mailbox hot loop");
        let lp = detect_natural_loop_at(&code, lp.header)
            .expect("loop should remain after checked payload pass");
        let (code, n_regs, _, _) =
            native_scalar_replace_results_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("result SR should accept selfhost mailbox hot loop");
        let lp =
            detect_natural_loop_at(&code, lp.header).expect("loop should remain after result SR");
        let (code, n_regs, _, _) =
            native_scalar_replace_options_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("option SR should accept selfhost mailbox hot loop");
        let lp =
            detect_natural_loop_at(&code, lp.header).expect("loop should remain after option SR");
        let (code, n_regs, _, _) =
            native_scalar_replace_variants_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("variant SR should accept selfhost mailbox hot loop");
        let lp =
            detect_natural_loop_at(&code, lp.header).expect("loop should remain after variant SR");
        let (code, n_regs, _, _) =
            native_scalar_replace_structs_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("struct SR should accept selfhost mailbox hot loop");
        let lp =
            detect_natural_loop_at(&code, lp.header).expect("loop should remain after struct SR");
        let (code, n_regs, _) =
            native_loop_carried_struct_in_region(&code, n_regs, lp.header, lp.exit)
                .unwrap_or_else(|| (code.clone(), n_regs, (0..code.len()).collect()));
        let lp = detect_natural_loop_at(&code, lp.header)
            .expect("loop should remain after loop-carried struct pass");
        translate_osr_loop(&code, n_regs, main_func.params, main_func.captures, lp).unwrap_or_else(
            || {
                panic!(
                    "selfhost mailbox hot loop should lower to OSR native IR; region={:#?}",
                    &code[lp.header..lp.exit],
                )
            },
        );
        main_func.osr_state.set(OsrTrigger::Unknown);
        let (_out, stats) = executable
            .eval_main_with_args_native_with_stats(["2000"])
            .expect("adaptive native selfhost mailbox run should succeed");
        assert!(
            stats.osr_entries > 0,
            "selfhost mailbox hot loop should OSR-enter; stats={stats:?}; osr_state={:?}",
            main_func.osr_state.get(),
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_scalar_replaces_whole_function_result() {
        let ok_layout = native_test_layout("Ok", &["value"]);
        let function = native_test_function(
            "result_hot",
            1,
            6,
            vec![
                RegInstr::LoadInt { dst: 1, value: 7 },
                RegInstr::MakeVariant {
                    dst: 2,
                    layout: Rc::clone(&ok_layout),
                    fields: vec![("value".to_string(), 0)],
                },
                RegInstr::MatchResult {
                    src: 2,
                    ok_ip: 3,
                    err_ip: 6,
                },
                RegInstr::UnwrapVariantValue {
                    dst: 3,
                    src: 2,
                    expected: "Ok".to_string(),
                },
                RegInstr::AddInt {
                    dst: 4,
                    lhs: 3,
                    rhs: 1,
                },
                RegInstr::Return { src: 4 },
                RegInstr::LoadInt { dst: 5, value: 0 },
                RegInstr::Return { src: 5 },
            ],
        );
        let unit = native_test_unit(vec![function]);

        assert!(
            translate_to_native_jit(&unit, unit.functions[0].as_ref()).is_some(),
            "whole-function native translation should dissolve always-Ok Results before subset checking",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_scalar_replaces_whole_function_option() {
        let function = native_test_function(
            "option_hot",
            1,
            6,
            vec![
                RegInstr::LoadInt { dst: 1, value: 7 },
                RegInstr::MakeSome { dst: 2, value: 0 },
                RegInstr::MatchOption {
                    src: 2,
                    some_ip: 3,
                    none_ip: 6,
                },
                RegInstr::UnwrapSome { dst: 3, src: 2 },
                RegInstr::AddInt {
                    dst: 4,
                    lhs: 3,
                    rhs: 1,
                },
                RegInstr::Return { src: 4 },
                RegInstr::LoadInt { dst: 5, value: 0 },
                RegInstr::Return { src: 5 },
            ],
        );
        let unit = native_test_unit(vec![function]);

        assert!(
            translate_to_native_jit(&unit, unit.functions[0].as_ref()).is_some(),
            "whole-function native translation should dissolve scalar Options before subset checking",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_scalar_replaces_whole_function_struct() {
        let struct_layout = native_test_layout("Pair", &["a", "b"]);
        let function = native_test_function(
            "struct_hot",
            1,
            6,
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
                RegInstr::GetFieldSlot {
                    dst: 4,
                    base: 2,
                    slot: 1,
                },
                RegInstr::AddInt {
                    dst: 5,
                    lhs: 3,
                    rhs: 4,
                },
                RegInstr::Return { src: 5 },
            ],
        );
        let unit = native_test_unit(vec![function]);

        assert!(
            translate_to_native_jit(&unit, unit.functions[0].as_ref()).is_some(),
            "whole-function native translation should dissolve structs before subset checking",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_sinks_whole_function_closure_allocation() {
        let callee = native_test_function(
            "mapper",
            1,
            4,
            vec![
                RegInstr::LoadInt { dst: 1, value: 2 },
                RegInstr::MulInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::LoadInt { dst: 3, value: 1 },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 2,
                    rhs: 3,
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
        let unit = native_test_unit(vec![callee, caller]);

        assert!(
            translate_to_native_jit(&unit, unit.functions[1].as_ref()).is_some(),
            "whole-function native translation should sink local closure allocation and inline the call",
        );
    }

