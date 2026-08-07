    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_scalar_replaces_whole_function_variant() {
        let variant_layout = native_test_layout("Boxed", &["value"]);
        let function = native_test_function(
            "variant_hot",
            1,
            6,
            vec![
                RegInstr::LoadInt { dst: 1, value: 7 },
                RegInstr::MakeVariant {
                    dst: 2,
                    layout: Rc::clone(&variant_layout),
                    fields: vec![("value".to_string(), 0)],
                },
                RegInstr::MatchVariant {
                    src: 2,
                    expected: "Boxed".to_string(),
                    match_ip: 3,
                    else_ip: 6,
                },
                RegInstr::UnwrapVariantValue {
                    dst: 3,
                    src: 2,
                    expected: "Boxed".to_string(),
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
            "whole-function native translation should dissolve user variants before subset checking",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_elides_split_list_when_only_len_is_used() {
        let function = native_test_function(
            "split_len_hot",
            2,
            4,
            vec![
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::StringSplit,
                    args: vec![0, 1],
                    dst: 2,
                },
                RegInstr::ListLen { dst: 3, list: 2 },
                RegInstr::Return { src: 3 },
            ],
        );
        let unit = native_test_unit(vec![function]);
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("split+len should translate through the host intrinsic framework");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringSplitCount,
                    ..
                }
            )),
            "split followed only by List.len should lower to the non-allocating count helper",
        );
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringSplit,
                    ..
                }
            )),
            "the materializing StringSplit helper should be removed when only len is used",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_forwards_adjacent_flat_int_list_store_load() {
        let function = native_test_function(
            "flat_list_set_hot",
            2,
            5,
            vec![
                RegInstr::LoadInt { dst: 2, value: 0 },
                RegInstr::ListSet {
                    dst: 3,
                    list: 0,
                    index: 2,
                    value: 1,
                },
                RegInstr::ListGet {
                    dst: 4,
                    list: 0,
                    index: 2,
                },
                RegInstr::Return { src: 4 },
            ],
        );
        let unit = native_test_unit(vec![function]);
        let (jit, _, params, _, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("flat Int list set/get should translate");

        assert_eq!(params[0], NativeTy::FlatIntMut);
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListSetIntDirect { .. })),
            "List.set<Int> on a flat mutable list param should lower to direct write; jit code: {:#?}",
            jit.code,
        );
        assert!(
            !jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListGetIntDirect { .. })),
            "an adjacent List.get<Int> of the stored slot should not repeat the direct read; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::Move { dst: 4, src: 1 })),
            "the stored value should be forwarded to the List.get destination; jit code: {:#?}",
            jit.code,
        );
        let telemetry = tier::NativeCompileTelemetry::from_jit_function(&jit);
        assert_eq!(telemetry.direct_list_bounds_check_sites, 1);
        assert_eq!(telemetry.direct_list_store_load_forwarded_moves, 1);
        assert_eq!(telemetry.memoized_host_call_sites, 0);
        assert_eq!(telemetry.host_call_sites, 0);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_does_not_forward_list_store_when_result_clobbers_index() {
        let function = native_test_function(
            "flat_list_set_clobbers_index",
            2,
            4,
            vec![
                RegInstr::LoadInt { dst: 2, value: 1 },
                RegInstr::ListSet {
                    dst: 2,
                    list: 0,
                    index: 2,
                    value: 1,
                },
                RegInstr::ListGet {
                    dst: 3,
                    list: 0,
                    index: 2,
                },
                RegInstr::Return { src: 3 },
            ],
        );
        let unit = native_test_unit(vec![function]);
        let (jit, _, params, _, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("flat Int list set/get should translate");

        assert_eq!(params[0], NativeTy::FlatIntMut);
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListGetIntDirect { .. })),
            "the load must remain because ListSet overwrites the index register before it; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_forwards_adjacent_flat_float_list_store_load() {
        let function = native_test_function(
            "flat_float_list_set_hot",
            2,
            5,
            vec![
                RegInstr::LoadInt { dst: 2, value: 0 },
                RegInstr::ListGet {
                    dst: 4,
                    list: 0,
                    index: 2,
                },
                RegInstr::ListSet {
                    dst: 3,
                    list: 0,
                    index: 2,
                    value: 1,
                },
                RegInstr::ListGet {
                    dst: 4,
                    list: 0,
                    index: 2,
                },
                RegInstr::Return { src: 4 },
            ],
        );
        let mut unit = native_test_unit(vec![function]);
        unit.native_signatures.insert(
            "flat_float_list_set_hot".to_string(),
            RegNativeSignature {
                params: vec!["List<Float>".to_string(), "Float".to_string()],
                return_type: Some("Float".to_string()),
            },
        );
        let (jit, ret, params, _, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("flat Float list set/get should translate");

        assert_eq!(params[0], NativeTy::FlatFloatMut);
        assert_eq!(ret, NativeTy::Float);
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListSetFloatDirect { .. })),
            "List.set<Float> should remain a checked direct write; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code
                .iter()
                .filter(|instr| matches!(instr, vm_jit::JitInstr::ListGetFloatDirect { .. }))
                .count()
                == 1,
            "only the initial Float read should remain; the adjacent post-store read should be forwarded; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::Move { dst: 4, src: 1 })),
            "the stored Float should be forwarded to the load destination; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_pass_elides_readonly_full_list_slice_alias() {
        let code = vec![
            RegInstr::LoadInt { dst: 1, value: 0 },
            RegInstr::ListLen { dst: 2, list: 0 },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::ListSlice,
                args: vec![0, 1, 2],
                dst: 3,
            },
            RegInstr::Move { dst: 5, src: 3 },
            RegInstr::ListGet {
                dst: 4,
                list: 5,
                index: 1,
            },
            RegInstr::Return { src: 4 },
        ];

        let (folded, _, _) =
            native_elide_readonly_full_list_slices_in_region(&code, 6, 0, code.len())
                .expect("full read-only slice should be analyzable");

        assert!(
            !folded.iter().any(|instr| matches!(
                instr,
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                }
            )),
            "materializing List.slice should be removed: {folded:#?}",
        );
        assert!(
            matches!(folded[2], RegInstr::Move { dst: 3, src: 0 }),
            "full read-only slice should become a handle alias: {folded:#?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_pass_does_not_elide_full_list_slice_with_branch_conflicting_start() {
        let code = vec![
            RegInstr::JumpIfBool {
                cond: 6,
                expected: false,
                target: 3,
            },
            RegInstr::LoadInt { dst: 1, value: 5 },
            RegInstr::Jump { target: 4 },
            RegInstr::LoadInt { dst: 1, value: 0 },
            RegInstr::ListLen { dst: 2, list: 0 },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::ListSlice,
                args: vec![0, 1, 2],
                dst: 3,
            },
            RegInstr::ListGet {
                dst: 4,
                list: 3,
                index: 1,
            },
            RegInstr::Return { src: 4 },
        ];

        let (folded, _, _) =
            native_elide_readonly_full_list_slices_in_region(&code, 7, 0, code.len())
                .expect("region should remain analyzable");

        assert!(
            matches!(
                folded[5],
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                }
            ),
            "slice must not be replaced by a whole-list alias when branch facts disagree: {folded:#?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_region_cfg_dedups_and_filters_successors() {
        let code = vec![
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 1,
            },
            RegInstr::Jump { target: 3 },
            RegInstr::Return { src: 0 },
            RegInstr::LoadInt { dst: 1, value: 7 },
        ];

        let cfg = NativeRegionCfg::new(&code, 0, 3).expect("valid region");

        assert_eq!(
            cfg.successors(0).unwrap(),
            &[1],
            "branch target equal to fallthrough should be deduplicated",
        );
        assert_eq!(
            cfg.successors(1).unwrap(),
            &[] as &[usize],
            "successor outside the region must be filtered",
        );
        assert_eq!(cfg.successors(2).unwrap(), &[] as &[usize]);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_subset_descriptor_has_precise_footprints() {
        let instructions = vec![
            RegInstr::LoadInt { dst: 0, value: 1 },
            RegInstr::LoadFloat { dst: 0, value: 1.0 },
            RegInstr::LoadBool {
                dst: 0,
                value: true,
            },
            RegInstr::LoadString {
                dst: 0,
                value: Rc::new("x".to_string()),
            },
            RegInstr::Move { dst: 0, src: 1 },
            RegInstr::DeepCopy { reg: 0 },
            RegInstr::AddInt {
                dst: 0,
                lhs: 1,
                rhs: 2,
            },
            RegInstr::Jump { target: 1 },
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 2,
            },
            RegInstr::JumpIfIntCompare {
                lhs: 0,
                rhs: 1,
                op: RegIntCompare::Less,
                expected: true,
                target: 2,
            },
            RegInstr::Return { src: 0 },
            RegInstr::RuntimeError {
                message: "boom".to_string(),
            },
            RegInstr::StringConcat {
                dst: 0,
                left: 1,
                right: 2,
            },
            RegInstr::GetFieldSlot {
                dst: 0,
                base: 1,
                slot: 0,
            },
            RegInstr::SetFieldSlot {
                dst: 0,
                base: 1,
                slot: 0,
                value: 2,
            },
            RegInstr::ListLen { dst: 0, list: 1 },
            RegInstr::ListGet {
                dst: 0,
                list: 1,
                index: 2,
            },
            RegInstr::ListSet {
                dst: 0,
                list: 1,
                index: 2,
                value: 3,
            },
            RegInstr::ListPush {
                dst: 0,
                list: 1,
                value: 2,
            },
            RegInstr::ListSort { dst: 0, list: 1 },
            RegInstr::MapInsert {
                dst: 0,
                map: 1,
                key: 2,
                value: 3,
            },
            RegInstr::SetInsert {
                dst: 0,
                set: 1,
                value: 2,
            },
            RegInstr::SortedSetInsert {
                dst: 0,
                set: 1,
                value: 2,
            },
            RegInstr::SortedMapInsert {
                dst: 0,
                map: 1,
                key: 2,
                value: 3,
            },
            RegInstr::DequePushBack {
                dst: 0,
                deque: 1,
                value: 2,
            },
            RegInstr::DequePushFront {
                dst: 0,
                deque: 1,
                value: 2,
            },
            RegInstr::DequePopFront { dst: 0, deque: 1 },
            RegInstr::DequePopBack { dst: 0, deque: 1 },
            RegInstr::MatchMapGet {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 3,
                none_ip: 4,
            },
            RegInstr::MatchSortedMapGet {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 3,
                none_ip: 4,
            },
            RegInstr::NativeGuardClosureId {
                closure: 0,
                expected: 1,
            },
            RegInstr::NativeClosureId { dst: 0, closure: 1 },
            RegInstr::NativeClosureCapture {
                dst: 0,
                closure: 1,
                index: 0,
            },
            RegInstr::NativeFieldClosureId {
                dst: 0,
                base: 1,
                slot: 0,
            },
            RegInstr::NativeFieldClosureCapture {
                dst: 0,
                base: 1,
                slot: 0,
                index: 0,
            },
            RegInstr::CallIntrinsic {
                dst: 0,
                intrinsic: RegIntrinsic::IntToFloat,
                args: vec![1],
            },
            RegInstr::CallIntrinsic {
                dst: 0,
                intrinsic: RegIntrinsic::StringLen,
                args: vec![1],
            },
            RegInstr::CallTypedIntrinsic {
                dst: 0,
                intrinsic: RegIntrinsic::ListNew,
                type_arg: "Int".to_string(),
                args: vec![],
            },
        ];

        for instr in instructions {
            assert!(
                native_subset_instruction(&instr),
                "fixture should be in the native subset: {instr:?}",
            );
            assert!(
                matches!(instr_read_regs(&instr), RegFootprint::Some(_)),
                "native-subset reads must be precise: {instr:?}",
            );
            assert!(
                matches!(instr_written_reg(&instr), RegFootprint::Some(_)),
                "native-subset writes must be precise: {instr:?}",
            );
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_host_intrinsic_registry_matches_backend_helper_signatures() {
        let cases = [
            (
                RegIntrinsic::ListNew,
                Some("Int"),
                vm_jit::HostHelper::ListNewInt,
            ),
            (
                RegIntrinsic::StringFromInt,
                None,
                vm_jit::HostHelper::StringFromInt,
            ),
            (RegIntrinsic::StringLen, None, vm_jit::HostHelper::StringLen),
            (
                RegIntrinsic::StringSlice,
                None,
                vm_jit::HostHelper::StringSlice,
            ),
            (
                RegIntrinsic::StringPadLeft,
                None,
                vm_jit::HostHelper::StringPadLeft,
            ),
            (
                RegIntrinsic::StringSplit,
                None,
                vm_jit::HostHelper::StringSplit,
            ),
            (
                RegIntrinsic::StringStartsWith,
                None,
                vm_jit::HostHelper::StringStartsWith,
            ),
            (
                RegIntrinsic::ListIsEmpty,
                None,
                vm_jit::HostHelper::ListIsEmpty,
            ),
            (
                RegIntrinsic::JsonParseOk,
                None,
                vm_jit::HostHelper::JsonParse,
            ),
            (
                RegIntrinsic::JsonFieldOk,
                None,
                vm_jit::HostHelper::JsonField,
            ),
            (
                RegIntrinsic::JsonFieldIntOk,
                None,
                vm_jit::HostHelper::JsonFieldInt,
            ),
            (RegIntrinsic::BytesLen, None, vm_jit::HostHelper::BytesLen),
            (
                RegIntrinsic::BytesSlice,
                None,
                vm_jit::HostHelper::BytesSlice,
            ),
            (
                RegIntrinsic::SetContains,
                None,
                vm_jit::HostHelper::MapContainsInt,
            ),
            (
                RegIntrinsic::MapIsEmpty,
                None,
                vm_jit::HostHelper::MapIsEmpty,
            ),
            (RegIntrinsic::MapLen, None, vm_jit::HostHelper::MapLen),
            (
                RegIntrinsic::SetIsEmpty,
                None,
                vm_jit::HostHelper::SetIsEmpty,
            ),
            (RegIntrinsic::SetLen, None, vm_jit::HostHelper::SetLen),
            (
                RegIntrinsic::SortedSetContains,
                None,
                vm_jit::HostHelper::SortedSetContainsInt,
            ),
            (
                RegIntrinsic::SortedSetIsEmpty,
                None,
                vm_jit::HostHelper::SortedSetIsEmpty,
            ),
            (
                RegIntrinsic::SortedSetLen,
                None,
                vm_jit::HostHelper::ListLen,
            ),
            (
                RegIntrinsic::SortedMapContainsKey,
                None,
                vm_jit::HostHelper::SortedMapContainsKeyInt,
            ),
            (
                RegIntrinsic::SortedMapIsEmpty,
                None,
                vm_jit::HostHelper::SortedMapIsEmpty,
            ),
            (
                RegIntrinsic::SortedMapLen,
                None,
                vm_jit::HostHelper::SortedMapLen,
            ),
            (RegIntrinsic::DequeLen, None, vm_jit::HostHelper::DequeLen),
            (
                RegIntrinsic::DequeIsEmpty,
                None,
                vm_jit::HostHelper::DequeIsEmpty,
            ),
        ];

        for (intrinsic, type_arg, expected_helper) in cases {
            let spec = native_host_typed_intrinsic(intrinsic, type_arg)
                .unwrap_or_else(|| panic!("{intrinsic:?}/{type_arg:?} should be native-lowered"));
            assert_eq!(spec.helper, expected_helper, "{intrinsic:?}/{type_arg:?}");
            assert_eq!(
                spec.helper.arg_types(),
                spec.arg_tys()
                    .iter()
                    .map(|ty| ty.jit_value_type())
                    .collect::<Vec<_>>(),
                "{intrinsic:?}/{type_arg:?} arg types must match backend helper signature",
            );
            assert_eq!(
                spec.helper.result_type(),
                Some(spec.result_ty.jit_value_type()),
                "{intrinsic:?}/{type_arg:?} result type must match backend helper signature",
            );
            assert_eq!(
                spec.produces_output_handle(),
                spec.helper.heap_effect().produces_heap_result(),
                "{intrinsic:?}/{type_arg:?} heap-result flag must match backend helper effect",
            );
            assert_eq!(
                spec.consumes_output_handles(),
                spec.helper
                    .arg_types()
                    .iter()
                    .any(|ty| *ty == vm_jit::JitValueType::Handle),
                "{intrinsic:?}/{type_arg:?} handle-input flag must match backend helper args",
            );
        }

        let concat = native_string_concat_host();
        assert_eq!(concat.helper, vm_jit::HostHelper::StringConcat);
        assert_eq!(
            concat.helper.result_type(),
            Some(concat.result_ty.jit_value_type())
        );
        assert_eq!(
            concat.produces_output_handle(),
            concat.helper.heap_effect().produces_heap_result()
        );
        assert_eq!(
            concat.consumes_output_handles(),
            concat
                .helper
                .arg_types()
                .iter()
                .any(|ty| *ty == vm_jit::JitValueType::Handle)
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_region_liveness_tracks_branch_sensitive_live_ins() {
        let code = vec![
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 2,
            },
            RegInstr::LoadInt { dst: 1, value: 7 },
            RegInstr::AddInt {
                dst: 2,
                lhs: 1,
                rhs: 3,
            },
            RegInstr::Return { src: 2 },
        ];

        let analysis =
            NativeRegionAnalysis::compute_region(&code, 4, 0, code.len()).expect("valid region");

        assert_eq!(
            analysis.live_in(0, 0),
            Some(true),
            "branch condition is read by the branch instruction",
        );
        assert_eq!(
            analysis.live_out(0, 1),
            Some(true),
            "reg 1 must remain live on the branch edge that skips its local definition",
        );
        assert_eq!(
            analysis.live_in(2, 1),
            Some(true),
            "AddInt reads reg 1 at the join",
        );
        assert_eq!(
            analysis.live_out(2, 1),
            Some(false),
            "reg 1 is dead after the AddInt consumes it",
        );
        assert_eq!(
            analysis.live_out(2, 2),
            Some(true),
            "AddInt's result is live into the Return",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn profile_cold_blocks_ignore_unreachable_branch_profiles() {
        let code = vec![
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 2,
            },
            RegInstr::LoadInt { dst: 1, value: 7 },
            RegInstr::Return { src: 1 },
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 5,
            },
            RegInstr::LoadInt { dst: 2, value: 13 },
            RegInstr::Return { src: 2 },
        ];
        let analysis =
            NativeRegionAnalysis::compute_prefix(&code, 3, 0, code.len()).expect("valid region");
        let mut profile = FunctionProfile::default();
        profile.branch_sites.insert(
            0,
            BranchFeedback {
                taken: PROFILE_BRANCH_MIN_SAMPLES,
                fallthrough: 0,
            },
        );
        profile.branch_sites.insert(
            3,
            BranchFeedback {
                taken: 0,
                fallthrough: PROFILE_BRANCH_MIN_SAMPLES,
            },
        );
        let ip_map: Vec<usize> = (0..code.len()).collect();

        let guidance = analysis.profile_guidance(&code, &profile, &ip_map);
        assert_eq!(
            guidance.cold_blocks,
            vec![1],
            "only the reachable branch's cold edge should affect profile-guided layout",
        );
        assert_eq!(
            guidance.hot_branch_edges.get(&0),
            Some(&true),
            "reachable branch profile should expose the hot target edge",
        );
        assert!(
            !guidance.hot_branch_edges.contains_key(&3),
            "unreachable branch profile must not drive profile-guided side exits",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_branch_profile_pending_uses_reachable_cfg_branches() {
        fn test_func(code: Vec<RegInstr>) -> RegFunction {
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

        let reachable_branch = test_func(vec![
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 2,
            },
            RegInstr::Return { src: 0 },
            RegInstr::Return { src: 0 },
        ]);
        assert!(
            native_scalar_callee_pending_on_branch_profile(&reachable_branch),
            "a reachable conditional branch should still wait for branch-profile warmup",
        );

        let unreachable_branch = test_func(vec![
            RegInstr::Jump { target: 2 },
            RegInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 3,
            },
            RegInstr::Return { src: 0 },
            RegInstr::Return { src: 0 },
        ]);
        assert!(
            !native_scalar_callee_pending_on_branch_profile(&unreachable_branch),
            "dead conditional branches must not stall native-call precompilation",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_option_scalar_replacement_requires_def_on_all_paths_before_read() {
        let code = vec![
            RegInstr::JumpIfBool {
                cond: 6,
                expected: true,
                target: 2,
            },
            RegInstr::Jump { target: 3 },
            RegInstr::MakeSome { dst: 1, value: 0 },
            RegInstr::MatchOption {
                src: 1,
                some_ip: 4,
                none_ip: 5,
            },
            RegInstr::UnwrapSome { dst: 2, src: 1 },
            RegInstr::Return { src: 0 },
        ];

        assert!(
            native_scalar_replace_options_in_region(&code, 7, 0, code.len()).is_none(),
            "Option SR must reject regions where any CFG path reads an Option before an in-region def",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_variant_region_scalar_replacement_emits_live_after_recipe() {
        let variant_layout = native_test_layout("Boxed", &["value"]);
        let code = vec![
            RegInstr::LoadInt { dst: 0, value: 7 },
            RegInstr::MakeVariant {
                dst: 2,
                layout: Rc::clone(&variant_layout),
                fields: vec![("value".to_string(), 0)],
            },
            RegInstr::MatchVariant {
                src: 2,
                expected: "Boxed".to_string(),
                match_ip: 3,
                else_ip: 4,
            },
            RegInstr::UnwrapVariantValue {
                dst: 3,
                src: 2,
                expected: "Boxed".to_string(),
            },
            RegInstr::Return { src: 2 },
        ];

        let (_, _, _, recipes) = native_scalar_replace_variants_in_region(&code, 5, 1, 4)
            .expect("variant SR should describe a reconstructible live-after value");
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].dst_reg, 2);
        assert!(matches!(
            &recipes[0].value,
            OsrMaterializeValue::Variant {
                tag_reg: Some(_),
                arms,
            } if arms.len() == 1
                && arms[0].layout.name.as_ref() == "Boxed"
                && matches!(arms[0].fields.as_slice(), [OsrMaterializeValue::Register(_)])
        ));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_aggregate_region_scalar_replacement_rejects_external_write() {
        let ok_layout = native_test_layout("Ok", &["value"]);
        let code = vec![
            RegInstr::LoadInt { dst: 0, value: 7 },
            RegInstr::MakeVariant {
                dst: 2,
                layout: Rc::clone(&ok_layout),
                fields: vec![("value".to_string(), 0)],
            },
            RegInstr::MatchResult {
                src: 2,
                ok_ip: 3,
                err_ip: 4,
            },
            RegInstr::UnwrapVariantValue {
                dst: 3,
                src: 2,
                expected: "Ok".to_string(),
            },
            RegInstr::Move { dst: 2, src: 0 },
        ];

        assert!(
            native_scalar_replace_results_in_region(&code, 5, 1, 4).is_none(),
            "result SR must reject regions whose original aggregate register is written outside the region",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_pass_does_not_elide_full_list_slice_with_branch_conflicting_len_source() {
        let code = vec![
            RegInstr::JumpIfBool {
                cond: 6,
                expected: false,
                target: 3,
            },
            RegInstr::ListLen { dst: 2, list: 0 },
            RegInstr::Jump { target: 4 },
            RegInstr::ListLen { dst: 2, list: 5 },
            RegInstr::LoadInt { dst: 1, value: 0 },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::ListSlice,
                args: vec![0, 1, 2],
                dst: 3,
            },
            RegInstr::ListGet {
                dst: 4,
                list: 3,
                index: 1,
            },
            RegInstr::Return { src: 4 },
        ];

        let (folded, _, _) =
            native_elide_readonly_full_list_slices_in_region(&code, 7, 0, code.len())
                .expect("region should remain analyzable");

        assert!(
            matches!(
                folded[5],
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::ListSlice,
                    ..
                }
            ),
            "slice must not be replaced when branch facts disagree on List.len source: {folded:#?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_elides_source_split_len_hot_function() {
        let source = r#"
fn hot(line: read String, delimiter: read String, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        let parts = String.split(value: read line, delimiter: read delimiter)
        total = total + List.len<String>(list: read parts)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    let total = hot(line: read "a,b,c", delimiter: read ",", limit: 10)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = crate::hir::Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[hot].as_ref())
            .expect("source split+len hot function should translate");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringSplitCount,
                    ..
                } | vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::StringSplitCount,
                    ..
                }
            )),
            "source split+len should lower to StringSplitCount; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_elides_pad_left_string_when_only_len_is_used() {
        let function = native_test_function(
            "pad_left_len_hot",
            3,
            5,
            vec![
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::StringPadLeft,
                    args: vec![0, 1, 2],
                    dst: 3,
                },
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::StringLen,
                    args: vec![3],
                    dst: 4,
                },
                RegInstr::Return { src: 4 },
            ],
        );
        let unit = native_test_unit(vec![function]);
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("pad_left+len should translate through the host intrinsic framework");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringPadLeftLen,
                    ..
                }
            )),
            "pad_left followed only by String.len should lower to the non-allocating length helper",
        );
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringPadLeft,
                    ..
                }
            )),
            "the materializing StringPadLeft helper should be removed when only len is used",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_elides_source_pad_left_len_hot_function() {
        let source = r#"
fn hot(line: read String, fill: read String, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        let padded = String.pad_left(value: read line, width: 2, fill: read fill)
        total = total + String.len(value: read padded)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    let total = hot(line: read "a", fill: read "é", limit: 10)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = crate::hir::Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[hot].as_ref())
            .expect("source pad_left+len hot function should translate");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringPadLeftLen,
                    ..
                } | vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::StringPadLeftLen,
                    ..
                }
            )),
            "source pad_left+len should lower to StringPadLeftLen; jit code: {:#?}",
            jit.code,
        );
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringPadLeft,
                    ..
                }
            )),
            "source pad_left+len should not materialize the padded string",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_memoizes_invariant_string_helpers_in_loop() {
        let source = r#"
fn hot(line: read String, delimiter: read String, fill: read String, prefix: read String, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        let parts = String.split(value: read line, delimiter: read delimiter)
        total = total + List.len<String>(list: read parts)
        let padded = String.pad_left(value: read line, width: 40, fill: read fill)
        total = total + String.len(value: read padded)
        if String.starts_with(value: read line, prefix: read prefix) {
            total = total + 1
        }
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    let total = hot(line: read "alpha,beta,gamma", delimiter: read ",", fill: read "0", prefix: read "alpha", limit: 10)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = crate::hir::Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[hot].as_ref())
            .expect("invariant string helper loop should translate");

        for helper in [
            vm_jit::HostHelper::StringSplitCount,
            vm_jit::HostHelper::StringPadLeftLen,
            vm_jit::HostHelper::StringStartsWith,
        ] {
            assert!(
                jit.code.iter().any(|instr| matches!(
                    instr,
                    vm_jit::JitInstr::MemoizedHostCall { helper: h, .. } if *h == helper
                )),
                "{helper:?} should lower to a memoized loop-invariant helper; jit code: {:#?}",
                jit.code,
            );
        }
        let telemetry = tier::NativeCompileTelemetry::from_jit_function(&jit);
        assert_eq!(telemetry.memoized_host_call_sites, 3);
        assert_eq!(telemetry.host_call_sites, 0);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_compile_shape_telemetry_is_visible_in_summary_and_json() {
        let stats = NativeStats {
            baseline_compiles: 2,
            optimized_compiles: 1,
            baseline_calls: 8,
            optimized_calls: 13,
            promotions: 1,
            admission_admitted: 5,
            admission_admitted_bytes: 4096,
            admission_rejected: 3,
            admission_rejected_bytes: 512,
            direct_list_bounds_check_sites: 4,
            memoized_host_call_sites: 3,
            host_call_sites: 2,
            fused_map_match_helper_sites: 1,
            direct_list_store_load_forwarded_moves: 1,
            ..NativeStats::default()
        };
        let json = stats.to_json();
        assert_eq!(json["baseline_compiles"].as_u64(), Some(2));
        assert_eq!(json["optimized_compiles"].as_u64(), Some(1));
        assert_eq!(json["baseline_calls"].as_u64(), Some(8));
        assert_eq!(json["optimized_calls"].as_u64(), Some(13));
        assert_eq!(json["promotions"].as_u64(), Some(1));
        assert_eq!(json["direct_list_bounds_check_sites"].as_u64(), Some(4));
        assert_eq!(json["memoized_host_call_sites"].as_u64(), Some(3));
        assert_eq!(json["host_call_sites"].as_u64(), Some(2));
        assert_eq!(json["fused_map_match_helper_sites"].as_u64(), Some(1));
        assert_eq!(json["admission_admitted"].as_u64(), Some(5));
        assert_eq!(json["admission_admitted_bytes"].as_u64(), Some(4096));
        assert_eq!(json["admission_rejected"].as_u64(), Some(3));
        assert_eq!(json["admission_rejected_bytes"].as_u64(), Some(512));
        assert_eq!(
            json["direct_list_store_load_forwarded_moves"].as_u64(),
            Some(1),
        );
        let summary = stats.summary();
        for field in [
            "baseline_compiles=2",
            "optimized_compiles=1",
            "baseline_calls=8",
            "optimized_calls=13",
            "promotions=1",
            "direct_list_bounds_check_sites=4",
            "memoized_host_call_sites=3",
            "host_call_sites=2",
            "fused_map_match_helper_sites=1",
            "direct_list_store_load_forwarded_moves=1",
            "admission_admitted=5",
            "admission_admitted_bytes=4096",
            "admission_rejected=3",
            "admission_rejected_bytes=512",
        ] {
            assert!(
                summary.contains(field),
                "text summary should expose {field}: {summary}",
            );
        }
    }
