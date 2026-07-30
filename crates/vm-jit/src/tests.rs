#[cfg(test)]
mod tests {
    use super::*;

    /// Test shim: validate as a non-OSR program (the common case for these IR
    /// validation tests). OSR-specific validation is exercised via `compile_osr`.
    fn validate(program: &JitFunction) -> Result<(), JitError> {
        super::validate(program, false)
    }

    #[test]
    fn execution_spec_tracks_the_public_ir_version() {
        let spec = include_str!("../../../docs/spec/RSScript_Execution_Spec_v0.1.md");
        assert!(
            spec.contains(&format!("`vm_jit::IR_VERSION`, currently `{IR_VERSION}`")),
            "execution spec must name vm-jit's current public IR version"
        );
    }

    #[test]
    fn host_helper_descriptor_table_is_complete_and_unique() {
        let helpers = HostHelper::all();
        assert_eq!(helpers.len(), HostHelper::DESCRIPTORS.len());
        let mut symbols = std::collections::HashSet::new();
        let mut seen_helpers = std::collections::HashSet::new();
        for (&helper, descriptor) in helpers.iter().zip(HostHelper::DESCRIPTORS.iter()) {
            assert_eq!(descriptor.helper, helper);
            assert!(seen_helpers.insert(helper), "duplicate helper: {helper:?}");
            assert!(
                symbols.insert(descriptor.symbol),
                "duplicate host helper symbol: {}",
                descriptor.symbol
            );
            assert_eq!(helper.symbol(), descriptor.symbol);
            assert_eq!(helper.signature().args, descriptor.sig.args);
            assert_eq!(helper.arg_types(), descriptor.sig.args);
            if helper.heap_effect().extends_input_handles() {
                assert!(
                    matches!(
                        helper.signature().result,
                        HostResult::Exact(JitValueType::Handle)
                    ),
                    "{helper:?} extends input handles but does not return a handle"
                );
            }
        }
        let found_out_helpers = helpers
            .iter()
            .copied()
            .filter(|helper| helper.signature().found_out)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            found_out_helpers,
            std::collections::HashSet::from([
                HostHelper::MapGetMatchInt,
                HostHelper::MapGetMatchFloat,
                HostHelper::SortedMapGetInt,
                HostHelper::SortedMapGetFloat,
            ]),
        );
    }

    #[test]
    fn generic_host_call_rejects_private_map_match_output() {
        use JitValueType::{Handle, Int};
        let program = ft(
            2,
            vec![Handle, Int, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::MapGetMatchInt,
                    dst: 2,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let error = validate(&program).expect_err("typed match instruction owns found output");
        assert!(error.0.contains("private found output"), "{error:?}");
    }

    #[test]
    fn host_helper_heap_effect_metadata_covers_transactional_helpers() {
        let mut mutates_input = std::collections::HashSet::from([
            HostHelper::ListSetInt,
            HostHelper::ListSetFloat,
            HostHelper::ListPushInt,
            HostHelper::ListPushHandle,
            HostHelper::ListPushFloat,
            HostHelper::ListSortInt,
            HostHelper::MapInsertInt,
            HostHelper::MapInsertHandleKeyInt,
            HostHelper::MapInsertFloat,
            HostHelper::SetInsertInt,
            HostHelper::SetInsertHandle,
            HostHelper::SortedSetInsertInt,
            HostHelper::SortedSetInsertHandle,
            HostHelper::SortedMapInsertInt,
            HostHelper::SortedMapInsertHandleKeyInt,
            HostHelper::DequePushBackInt,
            HostHelper::DequePushBackHandle,
            HostHelper::DequePushBackFloat,
            HostHelper::DequePushFrontInt,
            HostHelper::DequePushFrontHandle,
            HostHelper::DequePushFrontFloat,
            HostHelper::DequePopFrontInt,
            HostHelper::DequePopBackInt,
            HostHelper::DequePopFrontFloat,
            HostHelper::DequePopBackFloat,
        ]);
        let mut allocates_result = std::collections::HashSet::from([
            HostHelper::ListNewInt,
            HostHelper::StringFromInt,
            HostHelper::StringConcat,
            HostHelper::StringSlice,
            HostHelper::StringPadLeft,
            HostHelper::StringSplit,
            HostHelper::StringLiteral,
            HostHelper::JsonParse,
            HostHelper::JsonField,
            HostHelper::BytesSlice,
        ]);
        let mut extends_input_handles =
            std::collections::HashSet::from([HostHelper::FieldHandle, HostHelper::ListGetHandle]);

        for &helper in HostHelper::all() {
            let expected = if mutates_input.remove(&helper) {
                HostHeapEffect::MutatesInput
            } else if allocates_result.remove(&helper) {
                HostHeapEffect::AllocatesResult
            } else if extends_input_handles.remove(&helper) {
                HostHeapEffect::ExtendsInputHandles
            } else if helper == HostHelper::FieldSetInt
                || helper == HostHelper::FieldSetFloat
                || helper == HostHelper::FieldSetHandle
            {
                HostHeapEffect::ReplacesInput
            } else {
                HostHeapEffect::ReadOnly
            };
            assert_eq!(helper.heap_effect(), expected, "{helper:?}");
        }
        assert!(mutates_input.is_empty());
        assert!(allocates_result.is_empty());
        assert!(extends_input_handles.is_empty());
    }

    #[test]
    fn host_helper_projection_metadata_distinguishes_shape_writes() {
        let collection_len_readers = [
            HostHelper::ListLen,
            HostHelper::ListIsEmpty,
            HostHelper::MapLen,
            HostHelper::MapIsEmpty,
            HostHelper::SetLen,
            HostHelper::SetIsEmpty,
            HostHelper::SortedSetIsEmpty,
            HostHelper::SortedMapLen,
            HostHelper::SortedMapIsEmpty,
            HostHelper::DequeLen,
            HostHelper::DequeIsEmpty,
        ];
        for helper in collection_len_readers {
            assert_eq!(
                helper.heap_reads(),
                &[HostHeapAccess::new(0, HostHeapProjection::CollectionLen)],
                "{helper:?}"
            );
        }

        assert_eq!(
            HostHelper::ListSetInt.heap_writes(),
            &[HostHeapAccess::new(0, HostHeapProjection::Elements)]
        );
        assert!(
            HostHelper::MapInsertInt
                .heap_writes()
                .iter()
                .any(|access| access.projection == HostHeapProjection::CollectionLen)
        );
        for &helper in HostHelper::all() {
            if helper.heap_effect().writes_existing_heap() {
                assert!(
                    !helper.heap_writes().is_empty(),
                    "{helper:?} needs conservative write projection metadata"
                );
            }
        }
    }

    /// Test-only convenience over [`NativeModule::call`]: pass a zeroed `lens`
    /// (length-matched to `args`) for tests that use no flat-array params.
    trait CallScalar {
        fn callt(&self, id: CompiledId, args: &[i64]) -> Option<i64>;
    }
    impl CallScalar for NativeModule {
        fn callt(&self, id: CompiledId, args: &[i64]) -> Option<i64> {
            let lens = vec![0i64; args.len()];
            self.call(id, args, &lens).completed()
        }
    }

    /// `NativeOutcome`'s scalar/handle/any-bits accessors must keep a scalar result
    /// and an opaque heap-output-table handle distinct: `completed()` is scalar-only,
    /// `completed_handle()` is handle-only, and `completed_any_bits()` accepts either
    /// completed variant. A `Deopt` is `None` for all three.
    #[test]
    fn native_outcome_completed_accessors_separate_scalar_and_handle() {
        let scalar = NativeOutcome::Completed(10);
        assert_eq!(scalar.clone().completed(), Some(10));
        assert_eq!(scalar.clone().completed_handle(), None);
        assert_eq!(scalar.completed_any_bits(), Some(10));

        let handle = NativeOutcome::CompletedHandle(7);
        // A handle is NOT a scalar result, so `completed()` must hide it.
        assert_eq!(handle.clone().completed(), None);
        assert_eq!(handle.clone().completed_handle(), Some(7));
        assert_eq!(handle.completed_any_bits(), Some(7));

        let deopt = NativeOutcome::Deopt {
            safepoint_id: SafepointId(0),
            live: Vec::new(),
            child: None,
            logical_depth: None,
        };
        assert_eq!(deopt.clone().completed(), None);
        assert_eq!(deopt.clone().completed_handle(), None);
        assert_eq!(deopt.completed_any_bits(), None);
    }

    extern "C" fn noop_field_int(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_set_int(_ctx: HostCtx, _handle: i64, _slot: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_set_float(
        _ctx: HostCtx,
        _handle: i64,
        _slot: i64,
        _value: f64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_list_len(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_collection_len(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_is_empty(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_get_int(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_set_int(_ctx: HostCtx, _handle: i64, _index: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_set_float(
        _ctx: HostCtx,
        _handle: i64,
        _index: i64,
        _value: f64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_list_push_int(_ctx: HostCtx, _handle: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_push_float(_ctx: HostCtx, _handle: i64, _value: f64) -> i64 {
        0
    }
    extern "C" fn noop_list_sort_int(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_new_int(_ctx: HostCtx) -> i64 {
        0
    }
    extern "C" fn noop_field_float(_ctx: HostCtx, _handle: i64, _slot: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_list_get_float(_ctx: HostCtx, _handle: i64, _index: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_closure_id(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_closure_capture(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_closure_id(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_closure_capture(
        _ctx: HostCtx,
        _handle: i64,
        _slot: i64,
        _index: i64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_field_handle(_ctx: HostCtx, _handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_get_handle(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_from_int(_ctx: HostCtx, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_len(_ctx: HostCtx, _handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_concat(_ctx: HostCtx, _left: i64, _right: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_slice(_ctx: HostCtx, _value: i64, _start: i64, _len: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_pad_left(_ctx: HostCtx, _value: i64, _width: i64, _fill: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_pad_left_len(
        _ctx: HostCtx,
        _value: i64,
        _width: i64,
        _fill: i64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_string_split(_ctx: HostCtx, _value: i64, _delimiter: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_starts_with(_ctx: HostCtx, _value: i64, _prefix: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_split_count(_ctx: HostCtx, _value: i64, _delimiter: i64) -> i64 {
        0
    }
    extern "C" fn noop_string_literal(_ctx: HostCtx, _literal_id: i64) -> i64 {
        0
    }
    extern "C" fn noop_json_parse(_ctx: HostCtx, _text: i64) -> i64 {
        0
    }
    extern "C" fn noop_json_field(_ctx: HostCtx, _value: i64, _name: i64) -> i64 {
        0
    }
    extern "C" fn noop_json_field_int(_ctx: HostCtx, _value: i64, _name: i64) -> i64 {
        0
    }
    extern "C" fn noop_bytes_slice(_ctx: HostCtx, _value: i64, _start: i64, _len: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_insert_int(_ctx: HostCtx, _map: i64, _key: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_insert_float(_ctx: HostCtx, _map: i64, _key: i64, _value: f64) -> i64 {
        0
    }
    extern "C" fn noop_map_get_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_map_get_match_int(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        found: &mut i64,
    ) -> i64 {
        *found = 0;
        0
    }
    extern "C" fn noop_map_get_match_float(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        found: &mut i64,
    ) -> f64 {
        *found = 0;
        0.0
    }
    extern "C" fn noop_map_contains_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_set_insert_int(_ctx: HostCtx, _set: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_set_insert_int(_ctx: HostCtx, _set: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_set_contains_int(_ctx: HostCtx, _set: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_insert_int(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        _value: i64,
    ) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_get_int(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        found: &mut i64,
    ) -> i64 {
        *found = 0;
        0
    }
    extern "C" fn noop_sorted_map_get_float(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        found: &mut i64,
    ) -> f64 {
        *found = 0;
        0.0
    }
    extern "C" fn noop_sorted_map_contains_key_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        0
    }
    extern "C" fn noop_sorted_map_len(_ctx: HostCtx, _map: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_len(_ctx: HostCtx, _deque: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_push_back_int(_ctx: HostCtx, _deque: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_push_back_float(_ctx: HostCtx, _deque: i64, _value: f64) -> i64 {
        0
    }
    extern "C" fn noop_deque_push_front_int(_ctx: HostCtx, _deque: i64, _value: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_push_front_float(_ctx: HostCtx, _deque: i64, _value: f64) -> i64 {
        0
    }
    extern "C" fn noop_deque_pop_front_int(_ctx: HostCtx, _deque: i64) -> i64 {
        0
    }
    extern "C" fn noop_deque_pop_front_float(_ctx: HostCtx, _deque: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_deque_pop_back_float(_ctx: HostCtx, _deque: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_deque_pop_back_int(_ctx: HostCtx, _deque: i64) -> i64 {
        0
    }

    fn host_helpers() -> HostHelpers {
        HostHelpers {
            field_int: noop_field_int,
            field_set_int: noop_field_set_int,
            field_set_handle: noop_field_set_int,
            field_set_float: noop_field_set_float,
            list_len: noop_list_len,
            list_is_empty: noop_is_empty,
            list_get_int: noop_list_get_int,
            list_set_int: noop_list_set_int,
            list_set_float: noop_list_set_float,
            list_push_int: noop_list_push_int,
            list_push_handle: noop_list_push_int,
            list_push_float: noop_list_push_float,
            list_sort_int: noop_list_sort_int,
            list_new_int: noop_list_new_int,
            field_float: noop_field_float,
            list_get_float: noop_list_get_float,
            closure_id: noop_closure_id,
            closure_capture: noop_closure_capture,
            field_closure_id: noop_field_closure_id,
            field_closure_capture: noop_field_closure_capture,
            field_handle: noop_field_handle,
            list_get_handle: noop_list_get_handle,
            string_from_int: noop_string_from_int,
            string_len: noop_string_len,
            string_concat: noop_string_concat,
            string_slice: noop_string_slice,
            string_pad_left: noop_string_pad_left,
            string_pad_left_len: noop_string_pad_left_len,
            string_split: noop_string_split,
            string_starts_with: noop_string_starts_with,
            string_split_count: noop_string_split_count,
            string_literal: noop_string_literal,
            json_parse: noop_json_parse,
            json_field: noop_json_field,
            json_field_int: noop_json_field_int,
            bytes_len: noop_collection_len,
            bytes_slice: noop_bytes_slice,
            map_insert_int: noop_map_insert_int,
            map_insert_handle_key_int: noop_map_insert_int,
            map_insert_float: noop_map_insert_float,
            map_get_int: noop_map_get_int,
            map_get_match_int: noop_map_get_match_int,
            map_get_match_float: noop_map_get_match_float,
            map_contains_int: noop_map_contains_int,
            map_len: noop_collection_len,
            map_is_empty: noop_is_empty,
            set_insert_int: noop_set_insert_int,
            set_insert_handle: noop_set_insert_int,
            set_len: noop_collection_len,
            set_is_empty: noop_is_empty,
            sorted_set_insert_int: noop_sorted_set_insert_int,
            sorted_set_insert_handle: noop_sorted_set_insert_int,
            sorted_set_contains_int: noop_sorted_set_contains_int,
            sorted_set_is_empty: noop_is_empty,
            sorted_map_insert_int: noop_sorted_map_insert_int,
            sorted_map_insert_handle_key_int: noop_sorted_map_insert_int,
            sorted_map_get_int: noop_sorted_map_get_int,
            sorted_map_get_float: noop_sorted_map_get_float,
            sorted_map_contains_key_int: noop_sorted_map_contains_key_int,
            sorted_map_is_empty: noop_is_empty,
            sorted_map_len: noop_sorted_map_len,
            deque_len: noop_deque_len,
            deque_is_empty: noop_is_empty,
            deque_push_back_int: noop_deque_push_back_int,
            deque_push_back_handle: noop_deque_push_back_int,
            deque_push_back_float: noop_deque_push_back_float,
            deque_push_front_int: noop_deque_push_front_int,
            deque_push_front_handle: noop_deque_push_front_int,
            deque_push_front_float: noop_deque_push_front_float,
            deque_pop_front_int: noop_deque_pop_front_int,
            deque_pop_back_int: noop_deque_pop_back_int,
            deque_pop_front_float: noop_deque_pop_front_float,
            deque_pop_back_float: noop_deque_pop_back_float,
        }
    }

    /// A module with no-op host helpers (these tests exercise only scalar ops).
    fn module() -> NativeModule {
        NativeModule::new(host_helpers()).unwrap()
    }

    fn f(n_params: u32, n_regs: u32, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs,
            reg_types: vec![JitValueType::Int; n_regs as usize],
            zero_init_regs: Vec::new(),
            code,
            memo_scopes: Vec::new(),
            cold_blocks: Vec::new(),
        }
    }

    /// Like `f` but with explicit per-register storage classes (for float tests).
    fn ft(n_params: u32, reg_types: Vec<JitValueType>, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs: reg_types.len() as u32,
            reg_types,
            zero_init_regs: Vec::new(),
            code,
            memo_scopes: Vec::new(),
            cold_blocks: Vec::new(),
        }
    }

    #[test]
    fn rejects_memoized_heap_writing_host_helper() {
        use JitValueType::{Handle, Int};

        let err = validate(&ft(
            1,
            vec![Handle, Int, Int, Int],
            vec![
                JitInstr::MemoizedHostCall {
                    helper: HostHelper::ListSetInt,
                    dst: 3,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1), HostArg::Reg(2)],
                    memo_slot: 0,
                },
                JitInstr::Return { src: 3 },
            ],
        ))
        .expect_err("memoized heap-writing helper should be rejected");

        assert!(
            err.0.contains("heap-writing helpers cannot be memoized"),
            "{err:?}"
        );
    }

    // Heap-result return ABI: a function whose `Return` source is a
    // `Handle` register reports `CompletedHandle` carrying the i64 it returned (an
    // opaque output-table handle), while the scalar path stays `Completed`. This
    // pass-through (`fn(h) -> h`) performs no allocation/mutation; the host
    // materializes from its output table only on this clean completion (§7.2-safe).
    #[test]
    fn handle_returning_function_reports_completed_handle() {
        use JitValueType::Handle;
        let mut m = module();
        // fn(h: Handle) -> Handle { return h }
        let id = m
            .compile(&ft(1, vec![Handle], vec![JitInstr::Return { src: 0 }]))
            .unwrap();
        // The arg is an opaque table index (the host's input-table handle); the call
        // returns it verbatim via the heap-result variant.
        match m.call(id, &[7], &[0]) {
            NativeOutcome::CompletedHandle(h) => assert_eq!(h, 7),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
        // The scalar-return path is byte-identical: a plain Int return is `Completed`.
        let sid = m
            .compile(&f(1, 1, vec![JitInstr::Return { src: 0 }]))
            .unwrap();
        match m.call(sid, &[42], &[0]) {
            NativeOutcome::Completed(v) => assert_eq!(v, 42),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    static MEMOIZED_SPLIT_COUNT_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static MEMOIZED_SPLIT_COUNT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    extern "C" fn counting_string_split_count(_ctx: HostCtx, _value: i64, _delimiter: i64) -> i64 {
        MEMOIZED_SPLIT_COUNT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        5
    }

    #[test]
    fn memoized_host_call_reuses_first_scalar_result_in_loop() {
        use JitValueType::{Handle, Int};

        let _count_guard = MEMOIZED_SPLIT_COUNT_LOCK.lock().unwrap();
        MEMOIZED_SPLIT_COUNT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let mut helpers = host_helpers();
        helpers.string_split_count = counting_string_split_count;
        let mut m = NativeModule::new(helpers).unwrap();
        let mut program = ft(
            3,
            vec![Handle, Handle, Int, Int, Int, Int, Int],
            vec![
                JitInstr::LoadInt { dst: 3, value: 0 },
                JitInstr::LoadInt { dst: 4, value: 0 },
                JitInstr::JumpIfIntCompare {
                    lhs: 3,
                    rhs: 2,
                    op: JitCompare::Lt,
                    expected: false,
                    target: 8,
                },
                JitInstr::MemoizedHostCall {
                    helper: HostHelper::StringSplitCount,
                    dst: 5,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    memo_slot: 0,
                },
                JitInstr::Add {
                    dst: 4,
                    lhs: 4,
                    rhs: 5,
                },
                JitInstr::LoadInt { dst: 6, value: 1 },
                JitInstr::Add {
                    dst: 3,
                    lhs: 3,
                    rhs: 6,
                },
                JitInstr::Jump { target: 2 },
                JitInstr::Return { src: 4 },
            ],
        );
        program.memo_scopes.push(MemoScope {
            header: 2,
            exit: 8,
            memo_slots: vec![0],
        });
        let id = m.compile(&program).unwrap();
        assert_eq!(m.n_regs(id), Some(7));
        assert_eq!(m.deopt_map(id).unwrap().payload_words, 7);

        match m.call(id, &[10, 11, 4], &[0, 0, 0]) {
            NativeOutcome::Completed(v) => assert_eq!(v, 20),
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(
            MEMOIZED_SPLIT_COUNT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            1,
        );
    }

    fn nested_memo_program(osr: bool) -> JitFunction {
        use JitValueType::{Handle, Int};

        let mut program = ft(
            4,
            vec![Handle, Handle, Int, Int, Int, Int, Int, Int, Int],
            vec![
                JitInstr::LoadInt { dst: 4, value: 0 },
                JitInstr::LoadInt { dst: 5, value: 0 },
                JitInstr::JumpIfIntCompare {
                    lhs: 4,
                    rhs: 2,
                    op: JitCompare::Lt,
                    expected: false,
                    target: 13,
                },
                JitInstr::LoadInt { dst: 6, value: 0 },
                JitInstr::JumpIfIntCompare {
                    lhs: 6,
                    rhs: 3,
                    op: JitCompare::Lt,
                    expected: false,
                    target: 10,
                },
                JitInstr::MemoizedHostCall {
                    helper: HostHelper::StringSplitCount,
                    dst: 7,
                    args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    memo_slot: 0,
                },
                JitInstr::Add {
                    dst: 5,
                    lhs: 5,
                    rhs: 7,
                },
                JitInstr::LoadInt { dst: 8, value: 1 },
                JitInstr::Add {
                    dst: 6,
                    lhs: 6,
                    rhs: 8,
                },
                JitInstr::Jump { target: 4 },
                JitInstr::LoadInt { dst: 8, value: 1 },
                JitInstr::Add {
                    dst: 4,
                    lhs: 4,
                    rhs: 8,
                },
                JitInstr::Jump { target: 2 },
                if osr {
                    JitInstr::OsrExit
                } else {
                    JitInstr::Return { src: 5 }
                },
            ],
        );
        program.memo_scopes.push(MemoScope {
            header: 4,
            exit: 10,
            memo_slots: vec![0],
        });
        program
    }

    #[test]
    fn nested_memo_scope_resets_once_per_outer_activation_and_stays_lazy() {
        let _count_guard = MEMOIZED_SPLIT_COUNT_LOCK.lock().unwrap();
        MEMOIZED_SPLIT_COUNT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let mut helpers = host_helpers();
        helpers.string_split_count = counting_string_split_count;
        let mut module = NativeModule::new(helpers).unwrap();
        let id = module.compile(&nested_memo_program(false)).unwrap();

        assert_eq!(module.callt(id, &[10, 11, 3, 4]), Some(60));
        assert_eq!(
            MEMOIZED_SPLIT_COUNT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "the helper runs once for each activation of the inner loop"
        );

        MEMOIZED_SPLIT_COUNT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(module.callt(id, &[10, 11, 3, 0]), Some(0));
        assert_eq!(
            MEMOIZED_SPLIT_COUNT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "zero-iteration inner loops must not eagerly populate the memo"
        );
    }

    #[test]
    fn nested_memo_scope_osr_entry_resets_then_preserves_backedges() {
        let _count_guard = MEMOIZED_SPLIT_COUNT_LOCK.lock().unwrap();
        MEMOIZED_SPLIT_COUNT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        let mut helpers = host_helpers();
        helpers.string_split_count = counting_string_split_count;
        let mut module = NativeModule::new(helpers).unwrap();
        let program = nested_memo_program(true);
        let id = module.compile_osr(&program, 2, false, false).unwrap();
        let args = [10, 11, 3, 2, 0, 0, 0, 0, 0];
        let lens = [0; 9];

        assert!(matches!(
            module.call(id, &args, &lens),
            NativeOutcome::Deopt { .. }
        ));
        assert_eq!(
            MEMOIZED_SPLIT_COUNT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "OSR entry must begin with empty activation-local memo state"
        );
    }

    #[test]
    fn rejects_malformed_memo_scopes() {
        let mut unscoped = nested_memo_program(false);
        unscoped.memo_scopes.clear();
        let error = validate(&unscoped).expect_err("every memo slot needs a scope");
        assert!(error.0.contains("does not belong"), "{}", error.0);

        let mut interior_entry = nested_memo_program(false);
        interior_entry.code[0] = JitInstr::Jump { target: 5 };
        let error =
            validate(&interior_entry).expect_err("outside control flow cannot enter the interior");
        assert!(error.0.contains("enters scope interior"), "{}", error.0);

        let mut conditional_backedge = nested_memo_program(false);
        conditional_backedge.code[9] = JitInstr::JumpIfIntCompare {
            lhs: 6,
            rhs: 3,
            op: JitCompare::Lt,
            expected: true,
            target: 4,
        };
        let error =
            validate(&conditional_backedge).expect_err("conditional backedges are unsupported");
        assert!(
            error.0.contains("must be an unconditional Jump"),
            "{}",
            error.0
        );

        let mut bad_range = nested_memo_program(false);
        bad_range.memo_scopes[0].exit = bad_range.code.len() as u32;
        let error = validate(&bad_range).expect_err("scope exit must name an instruction");
        assert!(
            error.0.contains("header < exit < code length"),
            "{}",
            error.0
        );
    }

    // A forced bail of a handle-returning function reports `Deopt`, NOT
    // `CompletedHandle`: the heap result is materialized only on clean completion, so
    // a bailed attempt never reports a heap value (§7.2 no-effect-before-bail).
    #[test]
    fn handle_returning_function_bails_as_deopt_not_handle() {
        use JitValueType::Handle;
        let mut m = module();
        // fn(h: Handle) -> Handle { bail; return h } — force the (only) site to bail.
        let func = ft(
            1,
            vec![Handle],
            vec![JitInstr::Bail, JitInstr::Return { src: 0 }],
        );
        let id = m.compile(&func).unwrap();
        match m.call(id, &[7], &[0]) {
            NativeOutcome::Deopt { .. } => {}
            other => panic!("expected Deopt on bail, got {other:?}"),
        }
    }

    #[test]
    fn string_from_int_returns_heap_output_handle() {
        use JitValueType::{Handle, Int};
        extern "C" fn string_from_int(_ctx: HostCtx, value: i64) -> i64 {
            value + 100
        }
        let mut m = NativeModule::new(HostHelpers {
            string_from_int,
            ..host_helpers()
        })
        .unwrap();
        let id = m
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
        match m.call(id, &[23], &[0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 123),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
    }

    #[test]
    fn string_from_int_handle_can_feed_string_len() {
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
        let id = m
            .compile(&ft(
                1,
                vec![Int, Handle, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringFromInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
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
        match m.call(id, &[5], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 30),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn string_concat_returns_heap_output_handle() {
        use JitValueType::Handle;
        extern "C" fn string_concat(_ctx: HostCtx, left: i64, right: i64) -> i64 {
            left * 10 + right
        }
        let mut m = NativeModule::new(HostHelpers {
            string_concat,
            ..host_helpers()
        })
        .unwrap();
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Handle, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::StringConcat,
                        dst: 2,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        match m.call(id, &[4, 7], &[0, 1]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 47),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }
    }

    #[test]
    fn host_call_covers_heap_read_helpers_with_immediates_and_typed_results() {
        use JitValueType::{Float, Handle, Int};
        extern "C" fn field_int(_ctx: HostCtx, handle: i64, slot: i64) -> i64 {
            handle + slot
        }
        extern "C" fn list_len(_ctx: HostCtx, handle: i64) -> i64 {
            handle * 2
        }
        extern "C" fn list_get_int(_ctx: HostCtx, handle: i64, index: i64) -> i64 {
            handle + index * 3
        }
        extern "C" fn field_float(_ctx: HostCtx, handle: i64, slot: i64) -> f64 {
            handle as f64 + slot as f64 + 0.25
        }
        extern "C" fn list_get_float(_ctx: HostCtx, handle: i64, index: i64) -> f64 {
            handle as f64 + index as f64 + 0.5
        }
        extern "C" fn closure_capture(_ctx: HostCtx, _handle: i64, _index: i64) -> i64 {
            1.25_f64.to_bits() as i64
        }
        extern "C" fn field_handle(_ctx: HostCtx, handle: i64, slot: i64) -> i64 {
            handle * 10 + slot
        }
        extern "C" fn list_get_handle(_ctx: HostCtx, handle: i64, index: i64) -> i64 {
            handle * 100 + index
        }
        let mut m = NativeModule::new(HostHelpers {
            field_int,
            list_len,
            list_get_int,
            field_float,
            list_get_float,
            closure_capture,
            field_handle,
            list_get_handle,
            ..host_helpers()
        })
        .unwrap();

        let id = m
            .compile(&ft(
                1,
                vec![Handle, Int, Int, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldInt,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(2)],
                    },
                    JitInstr::HostCall {
                        helper: HostHelper::ListLen,
                        dst: 2,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::HostCall {
                        helper: HostHelper::ListGetInt,
                        dst: 3,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 3,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        match m.call(id, &[5], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 5 + (5 + 2) * 3 + 10),
            other => panic!("expected Completed, got {other:?}"),
        }

        let float_id = m
            .compile(&ft(
                1,
                vec![Handle, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldFloat,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(3)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(float_id, &[4], &[0]) {
            NativeOutcome::Completed(bits) => assert_eq!(f64::from_bits(bits as u64), 7.25),
            other => panic!("expected Completed float bits, got {other:?}"),
        }

        let list_float_id = m
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
        match m.call(list_float_id, &[4, 3], &[0, 0]) {
            NativeOutcome::Completed(bits) => assert_eq!(f64::from_bits(bits as u64), 7.5),
            other => panic!("expected Completed list float bits, got {other:?}"),
        }

        let capture_id = m
            .compile(&ft(
                1,
                vec![Handle, Float],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ClosureCapture,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(capture_id, &[9], &[0]) {
            NativeOutcome::Completed(bits) => assert_eq!(f64::from_bits(bits as u64), 1.25),
            other => panic!("expected Completed capture float bits, got {other:?}"),
        }

        let handle_id = m
            .compile(&ft(
                1,
                vec![Handle, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::FieldHandle,
                        dst: 1,
                        args: vec![HostArg::Reg(0), HostArg::ImmI64(6)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(handle_id, &[8], &[0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 86),
            other => panic!("expected CompletedHandle, got {other:?}"),
        }

        let list_handle_id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Handle],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ListGetHandle,
                        dst: 2,
                        args: vec![HostArg::Reg(0), HostArg::Reg(1)],
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        match m.call(list_handle_id, &[8, 6], &[0, 0]) {
            NativeOutcome::CompletedHandle(handle) => assert_eq!(handle, 806),
            other => panic!("expected list CompletedHandle, got {other:?}"),
        }
    }

    static MAP_GET_MATCH_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static MAP_GET_MATCH_FLOAT_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static SORTED_MAP_GET_INT_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static SORTED_MAP_GET_FLOAT_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    static MAP_CONTAINS_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn counting_map_get_match_int(
        _ctx: HostCtx,
        _map: i64,
        key: i64,
        found: &mut i64,
    ) -> i64 {
        MAP_GET_MATCH_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match key {
            7 => {
                *found = 1;
                0
            }
            8 => {
                *found = 1;
                123
            }
            _ => {
                *found = 0;
                0
            }
        }
    }

    extern "C" fn counting_map_contains_int(_ctx: HostCtx, _map: i64, _key: i64) -> i64 {
        MAP_CONTAINS_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        1
    }

    extern "C" fn counting_map_get_match_float(
        _ctx: HostCtx,
        _map: i64,
        key: i64,
        found: &mut i64,
    ) -> f64 {
        MAP_GET_MATCH_FLOAT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match key {
            7 => {
                *found = 1;
                0.0
            }
            8 => {
                *found = 1;
                -2.5
            }
            _ => {
                *found = 0;
                0.0
            }
        }
    }

    extern "C" fn counting_sorted_map_get_int(
        _ctx: HostCtx,
        _map: i64,
        key: i64,
        found: &mut i64,
    ) -> i64 {
        SORTED_MAP_GET_INT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if key == 4 {
            *found = 1;
            -77
        } else {
            *found = 0;
            0
        }
    }

    extern "C" fn counting_sorted_map_get_float(
        _ctx: HostCtx,
        _map: i64,
        key: i64,
        found: &mut i64,
    ) -> f64 {
        SORTED_MAP_GET_FLOAT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if key == 4 {
            *found = 1;
            -0.0
        } else {
            *found = 0;
            0.0
        }
    }

    extern "C" fn bailing_map_get_match_int(
        _ctx: HostCtx,
        _map: i64,
        _key: i64,
        found: &mut i64,
    ) -> i64 {
        *found = 1;
        signal_bail();
        99
    }

    #[test]
    fn match_map_get_uses_single_helper_boundary() {
        use JitValueType::{Handle, Int};

        MAP_GET_MATCH_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        MAP_CONTAINS_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);

        let mut helpers = host_helpers();
        helpers.map_get_match_int = counting_map_get_match_int;
        helpers.map_contains_int = counting_map_contains_int;
        let mut m = NativeModule::new(helpers).unwrap();
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Int],
                vec![
                    JitInstr::MatchMapGetInt {
                        map: 0,
                        key: 1,
                        value_dst: 2,
                        some_ip: 1,
                        none_ip: 3,
                    },
                    JitInstr::Return { src: 2 },
                    JitInstr::Nop,
                    JitInstr::LoadInt { dst: 2, value: -1 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        assert_eq!(m.call(id, &[42, 7], &[0, 0]).completed(), Some(0));
        assert_eq!(m.call(id, &[42, 8], &[0, 0]).completed(), Some(123));
        assert_eq!(m.call(id, &[42, 9], &[0, 0]).completed(), Some(-1));
        assert_eq!(
            MAP_GET_MATCH_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        assert_eq!(
            MAP_CONTAINS_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    #[test]
    fn fused_map_match_helpers_preserve_float_sorted_and_bail_semantics() {
        use JitValueType::{Float, Handle, Int};

        MAP_GET_MATCH_FLOAT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        SORTED_MAP_GET_INT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
        SORTED_MAP_GET_FLOAT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);

        let mut helpers = host_helpers();
        helpers.map_get_match_float = counting_map_get_match_float;
        helpers.sorted_map_get_int = counting_sorted_map_get_int;
        helpers.sorted_map_get_float = counting_sorted_map_get_float;
        let mut module = NativeModule::new(helpers).unwrap();

        let map_float = module
            .compile(&ft(
                2,
                vec![Handle, Int, Float],
                vec![
                    JitInstr::MatchMapGetFloat {
                        map: 0,
                        key: 1,
                        value_dst: 2,
                        some_ip: 1,
                        none_ip: 3,
                    },
                    JitInstr::Return { src: 2 },
                    JitInstr::Nop,
                    JitInstr::LoadFloat {
                        dst: 2,
                        value: 11.5,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        for (key, expected) in [(7, 0.0), (8, -2.5), (9, 11.5)] {
            let NativeOutcome::Completed(bits) = module.call(map_float, &[42, key], &[0, 0]) else {
                panic!("map float match should complete");
            };
            assert_eq!(f64::from_bits(bits as u64), expected);
        }

        let sorted_int = module
            .compile(&ft(
                2,
                vec![Handle, Int, Int],
                vec![
                    JitInstr::MatchSortedMapGetInt {
                        map: 0,
                        key: 1,
                        value_dst: 2,
                        some_ip: 1,
                        none_ip: 3,
                    },
                    JitInstr::Return { src: 2 },
                    JitInstr::Nop,
                    JitInstr::LoadInt { dst: 2, value: 5 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(
            module.call(sorted_int, &[42, 4], &[0, 0]).completed(),
            Some(-77)
        );
        assert_eq!(
            module.call(sorted_int, &[42, 5], &[0, 0]).completed(),
            Some(5)
        );

        let sorted_float = module
            .compile(&ft(
                2,
                vec![Handle, Int, Float],
                vec![
                    JitInstr::MatchSortedMapGetFloat {
                        map: 0,
                        key: 1,
                        value_dst: 2,
                        some_ip: 1,
                        none_ip: 3,
                    },
                    JitInstr::Return { src: 2 },
                    JitInstr::Nop,
                    JitInstr::LoadFloat {
                        dst: 2,
                        value: 8.25,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let NativeOutcome::Completed(bits) = module.call(sorted_float, &[42, 4], &[0, 0]) else {
            panic!("sorted float hit should complete");
        };
        assert_eq!(bits as u64, (-0.0f64).to_bits());
        let NativeOutcome::Completed(bits) = module.call(sorted_float, &[42, 5], &[0, 0]) else {
            panic!("sorted float miss should complete");
        };
        assert_eq!(f64::from_bits(bits as u64), 8.25);

        assert_eq!(
            MAP_GET_MATCH_FLOAT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            3
        );
        assert_eq!(
            SORTED_MAP_GET_INT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(
            SORTED_MAP_GET_FLOAT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
            2
        );

        let mut helpers = host_helpers();
        helpers.map_get_match_int = bailing_map_get_match_int;
        let mut module = NativeModule::new(helpers).unwrap();
        let bailing = module
            .compile(&ft(
                2,
                vec![Handle, Int, Int],
                vec![
                    JitInstr::MatchMapGetInt {
                        map: 0,
                        key: 1,
                        value_dst: 2,
                        some_ip: 1,
                        none_ip: 3,
                    },
                    JitInstr::Return { src: 2 },
                    JitInstr::Nop,
                    JitInstr::LoadInt { dst: 2, value: -1 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert!(matches!(
            module.call(bailing, &[42, 1], &[0, 0]),
            NativeOutcome::Deopt { .. }
        ));
    }

    #[test]
    fn host_call_closure_id_uses_non_bailing_failure_mode() {
        assert_eq!(
            HostHelper::ClosureId.signature().failure,
            HostFailureMode::CannotFail,
            "closure_id is total and must not inspect the shared bail flag",
        );
        use JitValueType::{Handle, Int};
        extern "C" fn closure_id(_ctx: HostCtx, handle: i64) -> i64 {
            handle + 4
        }
        let mut m = NativeModule::new(HostHelpers {
            closure_id,
            ..host_helpers()
        })
        .unwrap();
        let id = m
            .compile(&ft(
                1,
                vec![Handle, Int],
                vec![
                    JitInstr::HostCall {
                        helper: HostHelper::ClosureId,
                        dst: 1,
                        args: vec![HostArg::Reg(0)],
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        match m.call(id, &[9], &[0]) {
            NativeOutcome::Completed(value) => assert_eq!(value, 13),
            other => panic!("expected Completed, got {other:?}"),
        }
    }

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
            .compile(&ft(
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
                signal_bail();
                0.0
            }
        }
        extern "C" fn list_get_float(_ctx: HostCtx, _handle: i64, index: i64) -> f64 {
            if index >= 0 {
                index as f64 + 0.5
            } else {
                signal_bail();
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

    #[test]
    fn distinct_bail_sites_get_stable_safepoint_ids() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();

        // fn(a: FlatInt, x: Int, i: Int) -> Int { t = x + x; return a[i] }
        // Two distinct bail sites: the `Add` overflow guard (site 1) precedes the
        // `ListGetIntDirect` OOB guard (site 2). regs 0=a,1=x,2=i,3=t,4=res.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let ints: Vec<i64> = vec![10, 20, 30];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;

        // Bail at the FIRST site: x + x overflows, so the `Add` guard fires (id 1)
        // before the list read is ever reached.
        assert!(matches!(
            m.call_with_host_ctx(
                id,
                &[ints_ptr, i64::MAX, 0],
                &[ilen, 0, 0],
                0,
                &mut [FlatBufferArg::Int(&ints)]
            ),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(1),
                ..
            }
        ));
        // Pass the first guard (small x, no overflow) but bail at the SECOND site:
        // index 5 is out of bounds, so the direct-read OOB guard fires (id 2).
        assert!(matches!(
            m.call_with_host_ctx(
                id,
                &[ints_ptr, 1, 5],
                &[ilen, 0, 0],
                0,
                &mut [FlatBufferArg::Int(&ints)]
            ),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(2),
                ..
            }
        ));
        // Both guards pass → completes (id stays 0 = no bail recorded).
        assert!(matches!(
            m.call_with_host_ctx(
                id,
                &[ints_ptr, 1, 2],
                &[ilen, 0, 0],
                0,
                &mut [FlatBufferArg::Int(&ints)]
            ),
            NativeOutcome::Completed(_)
        ));
    }

    // --- J0.1a: deopt state-map (must-analysis) -------------------------------

    #[test]
    fn deopt_map_straightline_single_guard() {
        // fn(a, b) { t = a + b; return t }  regs 0=a,1=b,2=t. The `Add` (ip 0) has
        // one overflow guard (site id 1) → one site, resuming at ip 0 with the two
        // params live (t is not yet assigned on entry to its own instruction).
        let mut m = module();
        let id = m
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
        let map = m.deopt_map(id).expect("map for valid id");
        assert_eq!(map.sites.len(), 1);
        assert_eq!(map.sites[0].resume_ip, 0);
        assert_eq!(
            map.sites[0].live,
            vec![(0, JitValueType::Int), (1, JitValueType::Int)]
        );
    }

    #[test]
    fn profiled_branch_deopts_on_cold_edge() {
        let mut m = module();
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::ProfiledJumpIfIntCompare {
                        lhs: 0,
                        rhs: 1,
                        op: JitCompare::Lt,
                        expected: true,
                        target: 3,
                        hot_target: false,
                    },
                    JitInstr::LoadInt { dst: 2, value: 10 },
                    JitInstr::Return { src: 2 },
                    JitInstr::LoadInt { dst: 2, value: 99 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();

        assert_eq!(m.call(id, &[5, 3], &[0, 0]).completed(), Some(10));
        match m.call(id, &[1, 3], &[0, 0]) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(
                    live.iter().find(|reg| reg.reg == 0).map(|reg| reg.value),
                    Some(DeoptValue::Int(1))
                );
                assert_eq!(
                    live.iter().find(|reg| reg.reg == 1).map(|reg| reg.value),
                    Some(DeoptValue::Int(3))
                );
            }
            other => panic!("expected profiled cold edge to deopt, got {other:?}"),
        }
    }

    #[test]
    fn deopt_map_two_distinct_sites_track_prior_defs() {
        use JitValueType::{FlatInt, Int};
        // fn(a: FlatInt, x, i) { t = x + x; return a[i] }  regs 0=a,1=x,2=i,3=t,4=res.
        // Site 1: the `Add` overflow guard at ip 0 (t not yet live). Site 2: the
        // `ListGetIntDirect` OOB guard at ip 1 — by then `t` (reg 3) is definitely
        // assigned, so it appears in site 2's live set but not site 1's. (Mirrors
        // `distinct_bail_sites_get_stable_safepoint_ids`.)
        let mut m = module();
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        assert_eq!(map.sites.len(), 2);

        // Site 1 (id 1): resume at the Add (ip 0); params live, t (reg 3) is NOT.
        assert_eq!(map.sites[0].resume_ip, 0);
        assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
        // Params 0..3 are live (a is FlatInt, x/i are Int).
        assert_eq!(
            map.sites[0].live,
            vec![
                (0, JitValueType::FlatInt),
                (1, JitValueType::Int),
                (2, JitValueType::Int)
            ]
        );

        // Site 2 (id 2): resume at the direct read (ip 1); t (reg 3) is now live.
        assert_eq!(map.sites[1].resume_ip, 1);
        assert!(map.sites[1].live.contains(&(3, JitValueType::Int)));
    }

    #[test]
    fn deopt_map_must_analysis_excludes_one_armed_def() {
        // A register assigned on only ONE arm before a join with a guard must NOT be
        // in the join's live set (intersection / must-analysis).
        //
        //   0: if cond(reg1) goto 3            (cond is param reg 1)
        //   1:   t(reg3) = a(reg0) + a(reg0)   only the fall-through arm assigns t
        //   2:   goto 4
        //   3:   nop                           the taken arm leaves t unassigned
        //   4:   u(reg4) = a + a               guard here joins both arms
        //   5:   return u
        // regs: 0=a, 1=cond, 2=(unused scratch), 3=t, 4=u.
        use JitValueType::{Bool, Int};
        let mut m = module();
        let id = m
            .compile(&ft(
                2,
                vec![Int, Bool, Int, Int, Int],
                vec![
                    JitInstr::JumpIfBool {
                        cond: 1,
                        expected: true,
                        target: 3,
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 0,
                        rhs: 0,
                    },
                    JitInstr::Jump { target: 4 },
                    JitInstr::Nop,
                    JitInstr::Add {
                        dst: 4,
                        lhs: 0,
                        rhs: 0,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        // Two Add guards: site 1 at ip 1, site 2 at the post-join ip 4.
        assert_eq!(map.sites.len(), 2);
        assert_eq!(map.sites[0].resume_ip, 1);
        assert_eq!(map.sites[1].resume_ip, 4);
        // The key assertion: at the post-join guard (ip 4), `t` (reg 3) is assigned
        // on only one arm, so intersection excludes it from the live set.
        assert!(
            !map.sites[1].live.iter().any(|(r, _)| *r == 3),
            "reg 3 assigned on only one arm must not be live at the join: {:?}",
            map.sites[1].live
        );
        // The params (regs 0 and 1) are assigned on every path → still live.
        assert!(map.sites[1].live.contains(&(0, JitValueType::Int)));
        assert!(map.sites[1].live.contains(&(1, JitValueType::Bool)));
        // On the fall-through arm's own guard (ip 1) t is also not-yet live.
        assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
    }

    #[test]
    fn deopt_map_rejects_foreign_id() {
        // A foreign / out-of-range id yields no map, mirroring `call`'s validation.
        let mut m1 = module();
        let mut m2 = module();
        let id1 = m1.compile(&two_param_add()).unwrap();
        let _id2 = m2.compile(&two_param_add()).unwrap();
        assert!(m1.deopt_map(id1).is_some());
        assert!(m2.deopt_map(id1).is_none());
    }

    #[test]
    fn compiles_and_runs_add() {
        let mut m = module();
        // fn(a, b) { return a + b }   regs: 0=a,1=b,2=tmp
        let id = m
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
        assert_eq!(m.callt(id, &[3, 4]), Some(7));
        assert_eq!(m.callt(id, &[-10, 4]), Some(-6));
        // overflow bails:
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
    }

    #[test]
    fn loop_sum_to_n() {
        // fn(n) { total=0; i=1; while i<=n { total+=i; i+=1 } return total }
        // regs: 0=n, 1=total, 2=i, 3=one
        let mut m = module();
        let code = vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0
            JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
            JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
            // 3: loop head: if !(i<=n) goto end(8)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 0,
                op: JitCompare::Le,
                expected: false,
                target: 8,
            },
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 2,
            }, // 4 total+=i
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 3,
            }, // 5 i+=1
            JitInstr::Jump { target: 3 }, // 6 loop
            JitInstr::Nop,                // 7 (padding leader)
            JitInstr::Return { src: 1 },  // 8 end
        ];
        let id = m.compile(&f(1, 4, code)).unwrap();
        assert_eq!(m.callt(id, &[10]), Some(55));
        assert_eq!(m.callt(id, &[0]), Some(0));
        assert_eq!(m.callt(id, &[100]), Some(5050));
    }

    #[test]
    fn osr_entry_runs_loop_and_exits_with_live_out() {
        // Same loop as `loop_sum_to_n`, but compiled as an OSR-entry at the loop
        // header (ip 3) with the post-loop `Return` replaced by `OsrExit`. The
        // entry loads the live-in window (regs definitely-assigned at ip 3: total,
        // i, one, n) and jumps into the loop; the loop runs natively; on exit it
        // deopts at ip 8 with the live-out window (`total`).
        let mut m = module();
        let code = vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0 (pre-loop; not run under OSR)
            JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
            JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
            // 3: loop head: if !(i<=n) goto end(8)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 0,
                op: JitCompare::Le,
                expected: false,
                target: 8,
            },
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 2,
            }, // 4 total+=i
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 3,
            }, // 5 i+=1
            JitInstr::Jump { target: 3 }, // 6 loop
            JitInstr::Nop,                // 7 (padding leader)
            JitInstr::OsrExit,            // 8 OSR-exit (was Return)
        ];
        let prog = f(1, 4, code);
        let id = m.compile_osr(&prog, 3, false, false).unwrap();
        // The window is `n_regs`-wide, indexed by register. Seed the loop live-in:
        // total=0 (reg1), i=1 (reg2), one=1 (reg3), n (reg0). lens parallel & unused.
        let run = |n: i64| -> NativeOutcome {
            let window = [n, 0, 1, 1]; // reg0=n, reg1=total, reg2=i, reg3=one
            let lens = [0i64; 4];
            m.call(id, &window, &lens)
        };
        for &(n, expected) in &[(10i64, 55i64), (0, 0), (100, 5050)] {
            match run(n) {
                NativeOutcome::Deopt {
                    safepoint_id, live, ..
                } => {
                    let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                    assert_eq!(site.resume_ip, 8, "OSR-exit resumes at the post-loop ip");
                    let total = live
                        .iter()
                        .find(|r| r.reg == 1)
                        .map(|r| match r.value {
                            DeoptValue::Int(v) => v,
                            DeoptValue::Bool(_) => panic!("total is Int"),
                            DeoptValue::Float(_) => panic!("total is Int"),
                            DeoptValue::Handle(_) => panic!("total is Int"),
                        })
                        .expect("total is live-out");
                    assert_eq!(total, expected, "live-out total for n={n}");
                }
                NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => {
                    panic!("OSR loop must deopt at exit, not complete")
                }
            }
        }
    }

    #[test]
    fn osr_window_flat_list_direct_read_and_len() {
        // OSR loop summing a flat List<Int> that lives in a NON-param window slot
        // (reg index >= n_params). Models a loop-invariant typed list marshalled into
        // the OSR live-in window as a flat buffer: in-loop `List.get` lowers to
        // `ListGetIntDirect` and `List.len` to `ListLenDirect`, both basing off the
        // window register. This exercises the relaxed flat-base gate (admits an
        // OSR-window register, not only a top-level param).
        //
        // regs: 0=xs(FlatInt, NON-param window slot), 1=len, 2=i, 3=acc, 4=one, 5=elem
        // n_params=0. The flat list and every loop-carried value enter via the live-in
        // window (definite-assignment includes a register read-but-never-written in the
        // loop, exactly as `translate_osr_loop` produces for a pre-loop-built list whose
        // pre-header init instructions become `Bail` — no linear pred excludes it). The
        // pre-loop region is `Bail` (never run under OSR), so the header's only preds are
        // the loop backedge, keeping `xs`/`len`/`i`/`acc`/`one` definitely-assigned there.
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        let code = vec![
            JitInstr::Bail, // 0 pre-loop (never run under OSR; no successor)
            JitInstr::Bail, // 1
            JitInstr::Bail, // 2
            JitInstr::Bail, // 3
            // 4: loop head (OSR header): if !(i < len) goto end(9)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 1,
                op: JitCompare::Lt,
                expected: false,
                target: 9,
            },
            JitInstr::ListGetIntDirect {
                dst: 5,
                base: 0,
                index: 2,
            }, // 5 elem = xs[i]
            JitInstr::Add {
                dst: 3,
                lhs: 3,
                rhs: 5,
            }, // 6 acc += elem
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 4,
            }, // 7 i += 1
            JitInstr::Jump { target: 4 }, // 8 loop
            JitInstr::OsrExit,            // 9 exit (live-out: acc)
        ];
        let prog = ft(0, vec![FlatInt, Int, Int, Int, Int, Int], code);
        // OSR header at the loop head (ip 4). regs 0,1,2,3,4 are definitely assigned
        // there (read-only live-ins, never written in-loop).
        let id = m.compile_osr(&prog, 4, false, false).unwrap();

        let data: Vec<i64> = vec![10, 20, 30, 40];
        // The window is n_regs-wide; reg0's args slot holds the raw data pointer and
        // reg0's lens slot holds the element count (the flat-buffer ABI, by register).
        // Seed the loop live-in: len=4 (reg1), i=0 (reg2), acc=0 (reg3), one=1 (reg4).
        let mut window = [0i64; 6];
        window[0] = data.as_ptr() as i64;
        window[1] = data.len() as i64; // len (hoisted, live-in)
        window[2] = 0; // i
        window[3] = 0; // acc
        window[4] = 1; // one
        let mut lens = [0i64; 6];
        lens[0] = data.len() as i64;
        match m.call_with_host_ctx(id, &window, &lens, 0, &mut [FlatBufferArg::Int(&data)]) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                assert_eq!(site.resume_ip, 9, "exits at post-loop ip");
                let acc = live
                    .iter()
                    .find(|r| r.reg == 3)
                    .map(|r| match r.value {
                        DeoptValue::Int(v) => v,
                        DeoptValue::Bool(_) => panic!("acc is Int"),
                        DeoptValue::Float(_) => panic!("acc is Int"),
                        DeoptValue::Handle(_) => panic!("acc is Int"),
                    })
                    .expect("acc is live-out");
                assert_eq!(acc, 100, "sum of [10,20,30,40] via direct reads");
            }
            other => panic!("OSR loop must deopt at exit, got {other:?}"),
        }
        // OOB safety: a window whose lens claims more elements than the buffer has
        // would read OOB — but every direct read is bounds-checked against lens, so a
        // shorter real loop bound (len) keeps every index in range. To prove the OOB
        // guard itself bails, force an index past the buffer by lying about len upward
        // is unsound for the test buffer; instead, an empty buffer (len 0) must make
        // the very first `i < len` false and exit immediately with acc=0.
        let mut empty_window = [0i64; 6];
        let empty: Vec<i64> = vec![];
        empty_window[0] = empty.as_ptr() as i64;
        empty_window[1] = 0; // len = 0
        empty_window[4] = 1; // one
        let mut empty_lens = [0i64; 6];
        empty_lens[0] = 0;
        match m.call_with_host_ctx(
            id,
            &empty_window,
            &empty_lens,
            0,
            &mut [FlatBufferArg::Int(&empty)],
        ) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                assert_eq!(site.resume_ip, 9);
                let acc = live.iter().find(|r| r.reg == 3).map(|r| match r.value {
                    DeoptValue::Int(v) => v,
                    DeoptValue::Bool(_) => panic!(),
                    DeoptValue::Float(_) => panic!(),
                    DeoptValue::Handle(_) => panic!(),
                });
                assert_eq!(acc, Some(0), "empty list sums to 0");
            }
            other => panic!("empty OSR loop must deopt at exit, got {other:?}"),
        }

        // ListLenDirect in-loop off a NON-param flat window register + OOB direct read.
        // regs: 0=xs(FlatInt), 1=i, 2=acc, 3=one, 4=len, 5=elem. The loop reads len
        // directly each iteration (ListLenDirect base=0) and indexes xs[i]; an `i`
        // pushed past `len` (here we drive the loop bound from a SEPARATE register `b`
        // larger than the buffer) makes the direct read OOB ⇒ a bounds-check bail/deopt
        // (NOT UB), matching the host helper's OOB bail.
        // regs: 0=xs, 1=i, 2=acc, 3=one, 4=len(unused-here), 5=elem, 6=bound
        let code2 = vec![
            JitInstr::Bail, // 0 pre-loop
            JitInstr::Bail, // 1
            JitInstr::Bail, // 2
            JitInstr::Bail, // 3
            // 4: header: if !(i < bound) goto end(10)
            JitInstr::JumpIfIntCompare {
                lhs: 1,
                rhs: 6,
                op: JitCompare::Lt,
                expected: false,
                target: 10,
            },
            JitInstr::ListLenDirect { dst: 4, base: 0 }, // 5 len = len(xs)  (direct)
            JitInstr::ListGetIntDirect {
                dst: 5,
                base: 0,
                index: 1,
            }, // 6 elem = xs[i] (OOB once i>=len)
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 5,
            }, // 7 acc += elem
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 3,
            }, // 8 i += 1
            JitInstr::Jump { target: 4 },                // 9 loop
            JitInstr::OsrExit,                           // 10 exit
        ];
        let prog2 = ft(
            0,
            vec![JitValueType::FlatInt, Int, Int, Int, Int, Int, Int],
            code2,
        );
        let id2 = m.compile_osr(&prog2, 4, false, false).unwrap();
        // Drive bound=8 but the buffer only has 4 elements ⇒ at i==4 the direct read is
        // OOB and must deopt (a bounds bail), never reading past the buffer.
        let mut w2 = [0i64; 7];
        w2[0] = data.as_ptr() as i64; // xs
        w2[1] = 0; // i
        w2[2] = 0; // acc
        w2[3] = 1; // one
        w2[6] = 8; // bound (> len 4)
        let mut l2 = [0i64; 7];
        l2[0] = data.len() as i64; // lens[xs] = 4 — the bounds-check source
        match m.call_with_host_ctx(id2, &w2, &l2, 0, &mut [FlatBufferArg::Int(&data)]) {
            NativeOutcome::Deopt { safepoint_id, .. } => {
                let site = m.deopt_map(id2).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                // The OOB read bails at the ListGetIntDirect ip (6), NOT the exit (10):
                // a precise mid-loop deopt, so the interpreter re-runs and raises the
                // real out-of-bounds behavior itself.
                assert_eq!(
                    site.resume_ip, 6,
                    "OOB direct read bails at its own ip (not UB)"
                );
            }
            other => panic!("OOB direct read must deopt, got {other:?}"),
        }
    }

    #[test]
    fn osr_flat_base_gate_rejects_nonwindow_non_osr() {
        // The relaxed flat-base gate: under OSR a flat base may be any register in the
        // n_regs-wide window (index >= n_params); under a NORMAL compile it must still
        // be a packed param (index < n_params). A non-OSR program with a flat base at a
        // non-param register is rejected by validation.
        use JitValueType::{FlatInt, Int};
        // n_params=1 (reg0 the only param), reg2 is a FlatInt non-param ⇒ illegal base
        // for a normal compile.
        let prog = ft(
            1,
            vec![Int, Int, FlatInt, Int],
            vec![
                JitInstr::ListGetIntDirect {
                    dst: 1,
                    base: 2,
                    index: 0,
                },
                JitInstr::Return { src: 1 },
            ],
        );
        assert!(
            super::validate(&prog, false).is_err(),
            "non-param flat base must be rejected by a normal compile"
        );
        // Under OSR the same dataflow shape validates once it uses the OSR exit
        // contract (the window is n_regs-wide).
        let osr_prog = ft(
            1,
            vec![Int, Int, FlatInt, Int],
            vec![
                JitInstr::ListGetIntDirect {
                    dst: 1,
                    base: 2,
                    index: 0,
                },
                JitInstr::OsrExit,
            ],
        );
        assert!(
            super::validate(&osr_prog, true).is_ok(),
            "an OSR-window flat base (index >= n_params) must validate"
        );
    }

    #[test]
    fn osr_rejects_non_leader_header() {
        // An OSR header ip that is not a leader / jump-target block is rejected
        // cleanly (no panic, no miscompile).
        let mut m = module();
        let prog = f(
            1,
            2,
            vec![
                JitInstr::Add {
                    dst: 1,
                    lhs: 0,
                    rhs: 0,
                },
                JitInstr::Return { src: 1 },
            ],
        );
        // ip 1 (the Return) is a leader only if a jump targets it; here none does,
        // and it is not ip 0, so it has no block.
        assert!(m.compile_osr(&prog, 1, false, false).is_err());
    }

    #[test]
    fn div_by_zero_bails() {
        let mut m = module();
        let id = m
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
        assert_eq!(m.callt(id, &[20, 5]), Some(4));
        assert_eq!(m.callt(id, &[20, 0]), None);
        assert_eq!(m.callt(id, &[i64::MIN, -1]), None);
    }

    // --- J0.1b: live-register value capture at deopt --------------------------

    /// Find the captured value of register `reg` in a deopt outcome's `live` set.
    fn live_value(outcome: &NativeOutcome, reg: u32) -> Option<DeoptValue> {
        match outcome {
            NativeOutcome::Deopt { live, .. } => {
                live.iter().find(|r| r.reg == reg).map(|r| r.value)
            }
            NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => None,
        }
    }

    #[test]
    fn deopt_capture_records_live_register_values() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        // fn(xs: FlatInt, a: Int, b: Int) -> Int { t = a + b; return xs[t] }
        // regs 0=xs, 1=a, 2=b, 3=t. The `ListGetIntDirect` OOB guard (ip 1) resumes
        // with xs(0)/a(1)/b(2)/t(3) all definitely assigned on entry.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 3,
                        base: 0,
                        index: 3,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let xs: Vec<i64> = vec![10, 20, 30, 40, 50];
        let xs_ptr = xs.as_ptr() as i64;
        let xlen = xs.len() as i64;

        // In range: t = 1 + 2 = 3 → xs[3] = 40.
        assert_eq!(
            m.call_with_host_ctx(
                id,
                &[xs_ptr, 1, 2],
                &[xlen, 0, 0],
                0,
                &mut [FlatBufferArg::Int(&xs)]
            ),
            NativeOutcome::Completed(40)
        );

        // Out of range: t = 3 + 4 = 7 >= len 5 → the direct-read OOB guard bails.
        let out = m.call_with_host_ctx(
            id,
            &[xs_ptr, 3, 4],
            &[xlen, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&xs)],
        );
        assert!(matches!(out, NativeOutcome::Deopt { .. }));
        // t (reg 3) was computed before the guard fired and is captured.
        assert_eq!(live_value(&out, 3), Some(DeoptValue::Int(7)));
        // The params a (reg 1) and b (reg 2) are captured with their passed values.
        assert_eq!(live_value(&out, 1), Some(DeoptValue::Int(3)));
        assert_eq!(live_value(&out, 2), Some(DeoptValue::Int(4)));
    }

    #[test]
    fn deopt_capture_records_float_register_value() {
        use JitValueType::{FlatInt, Float, Int};
        let mut m = module();
        // fn(xs: FlatInt, i: Int, f: Float) -> Int { g = f + f; return xs[i] }
        // regs 0=xs, 1=i, 2=f, 3=g. The float `g` is definitely assigned before the
        // `ListGetIntDirect` OOB guard (ip 2), so it is captured as an exact f64.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Float, Float],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 2,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 1,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let xs: Vec<i64> = vec![7];
        let xs_ptr = xs.as_ptr() as i64;
        let xlen = xs.len() as i64;
        let f = 1.5_f64;

        // Out of range index 9 → bail; the float g = f + f = 3.0 round-trips exactly.
        let out = m.call_with_host_ctx(
            id,
            &[xs_ptr, 9, f.to_bits() as i64],
            &[xlen, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&xs)],
        );
        assert!(matches!(out, NativeOutcome::Deopt { .. }));
        assert_eq!(live_value(&out, 3), Some(DeoptValue::Float(f + f)));
        // The float param f itself is captured exactly too.
        assert_eq!(live_value(&out, 2), Some(DeoptValue::Float(f)));
    }

    // --- J0.3: deopt-at-every-safepoint stress test (master correctness) ------

    /// Force a bail at EVERY safepoint of a few representative functions and verify
    /// the captured safepoint id + live register values are correct at each — even at
    /// safepoints that never fire under the (deliberately in-range, non-overflowing)
    /// inputs. This exercises the J0 capture/map machinery exhaustively: for every
    /// site `k`, `compile_forcing_bail(f, k)` makes only site `k` bail, and we assert
    /// the outcome is `Deopt { SafepointId(k) }` whose `live` set is exactly the one
    /// `deopt_map().sites[k-1]` advertises, each register carrying the value the
    /// function computes for it. Late sites must capture earlier intermediates.
    #[test]
    fn force_bail_at_every_safepoint_captures_correct_state() {
        use JitValueType::{FlatInt, Float, Int};

        // A representative case: a `JitFunction`, the in-range inputs (args/lens) that
        // make NO natural bail fire, and a closure giving the value the function
        // computes for each register at any safepoint, so we can check every capture.
        struct Case {
            name: &'static str,
            func: JitFunction,
            args: Vec<i64>,
            lens: Vec<i64>,
            // reg -> captured DeoptValue, for any reg appearing in a site's live set.
            expect: Box<dyn Fn(u32) -> DeoptValue>,
        }

        let ints: Vec<i64> = vec![10, 20, 30, 40, 50];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;

        // Case A: fn(a: FlatInt, x: Int, i: Int) { t = x + x; return a[i] }
        // Sites: 1 = Add overflow guard (ip 0), 2 = ListGetIntDirect OOB (ip 1).
        // Site 2 is LATE and captures the earlier-computed t = x + x.
        let a_ptr = ints_ptr;
        let case_a = Case {
            name: "add-then-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ),
            // a = ptr, x = 7, i = 2 (in range). t = x + x = 14.
            args: vec![a_ptr, 7, 2],
            lens: vec![ilen, 0, 0],
            expect: Box::new(|reg| match reg {
                0 => DeoptValue::Int(0), // a: ptr value is asserted separately
                1 => DeoptValue::Int(7),
                2 => DeoptValue::Int(2),
                3 => DeoptValue::Int(14),
                _ => DeoptValue::Int(0),
            }),
        };

        // Case B: fn(xs: FlatInt, i: Int, f: Float) { g = f + f; return xs[i] }
        // The float Add has no guard, so the only site is the ListGetIntDirect OOB
        // (ip 1). Its live set includes the float register g — checked as exact f64.
        let xs_ptr = ints_ptr;
        let fv = 1.25_f64;
        let case_b = Case {
            name: "float-reg-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Float, Float],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 2,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 1,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ),
            // xs = ptr, i = 1 (in range), f = 1.25. g = f + f = 2.5.
            args: vec![xs_ptr, 1, fv.to_bits() as i64],
            lens: vec![ilen, 0, 0],
            expect: Box::new(move |reg| match reg {
                1 => DeoptValue::Int(1),
                2 => DeoptValue::Float(fv),
                3 => DeoptValue::Float(fv + fv),
                _ => DeoptValue::Int(0),
            }),
        };

        // Case C: fn(a: FlatInt, x: Int, y: Int) { p = x + y; q = p * x; return a[y] }
        // Three sites: 1 = Add (ip 0), 2 = Mul (ip 1), 3 = ListGetIntDirect OOB (ip 2).
        // The LATE site 3 captures both earlier intermediates p and q.
        let case_c = Case {
            name: "add-mul-then-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Mul {
                        dst: 4,
                        lhs: 3,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 5,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 5 },
                ],
            ),
            // a = ptr, x = 3, y = 4 (in range). p = 3 + 4 = 7, q = p * x = 21.
            args: vec![ints_ptr, 3, 4],
            lens: vec![ilen, 0, 0],
            expect: Box::new(|reg| match reg {
                1 => DeoptValue::Int(3),
                2 => DeoptValue::Int(4),
                3 => DeoptValue::Int(7),  // p
                4 => DeoptValue::Int(21), // q
                _ => DeoptValue::Int(0),
            }),
        };

        let cases = [case_a, case_b, case_c];
        let mut combinations = 0usize;
        let mut late_intermediate_checks = 0usize;

        for case in &cases {
            let mut m = module();
            // Site count from the natural (un-forced) compilation.
            let base_id = m.compile(&case.func).unwrap();
            let n = m.deopt_map(base_id).expect("map for valid id").sites.len();
            assert!(n >= 1, "{}: expected at least one safepoint", case.name);

            for k in 1..=n as u32 {
                // Force ONLY site k to bail; inputs are chosen so no natural bail fires.
                let id = m.compile_forcing_bail(&case.func, k).unwrap();
                let site = m.deopt_map(id).expect("map").sites[(k - 1) as usize].clone();
                let out = m.call_with_host_ctx(
                    id,
                    &case.args,
                    &case.lens,
                    0,
                    &mut [FlatBufferArg::Int(&ints)],
                );

                // The forced site must bail with exactly its id.
                let live = match &out {
                    NativeOutcome::Deopt {
                        safepoint_id, live, ..
                    } => {
                        assert_eq!(
                            *safepoint_id,
                            SafepointId(k),
                            "{}: forced site {} reported wrong safepoint id",
                            case.name,
                            k
                        );
                        live
                    }
                    NativeOutcome::Completed(_) | NativeOutcome::CompletedHandle(_) => {
                        panic!("{}: forced site {} did not bail", case.name, k)
                    }
                };

                // Heap-aware deopt (J0.1): the captured live set is exactly the SCALAR
                // (`Int`/`Float`) subset of the map's live set — `Handle`/`FlatInt`/
                // `FlatFloat` regs are reconstructed from the interpreter frame, not the
                // payload, so they are intentionally absent from the capture.
                let mut captured: Vec<u32> = live.iter().map(|r| r.reg).collect();
                captured.sort_unstable();
                let mut expected_regs: Vec<u32> = site
                    .live
                    .iter()
                    .filter(|(_, ty)| matches!(ty, JitValueType::Int | JitValueType::Float))
                    .map(|(r, _)| *r)
                    .collect();
                expected_regs.sort_unstable();
                assert_eq!(
                    captured, expected_regs,
                    "{}: site {} scalar live-reg set mismatch (map vs capture)",
                    case.name, k
                );

                // ...each captured SCALAR value must match what the function computes;
                // a non-scalar (Handle/FlatInt/FlatFloat) reg is NOT reconstructed and
                // must be absent from the capture.
                for &(reg, ty) in &site.live {
                    match ty {
                        JitValueType::Int | JitValueType::Bool | JitValueType::Float => {
                            let got = live_value(&out, reg).expect("captured scalar reg present");
                            assert_eq!(
                                got,
                                (case.expect)(reg),
                                "{}: site {} reg {} value mismatch",
                                case.name,
                                k,
                                reg
                            );
                        }
                        JitValueType::Handle
                        | JitValueType::FlatInt
                        | JitValueType::FlatIntMut
                        | JitValueType::FlatFloat
                        | JitValueType::FlatFloatMut => {
                            assert!(
                                live_value(&out, reg).is_none(),
                                "{}: site {} reg {} (non-scalar) must not be reconstructed",
                                case.name,
                                k,
                                reg
                            );
                        }
                    }
                }

                combinations += 1;
            }

            // Explicit late-site check: the LAST site of a multi-site function must
            // capture an earlier-computed intermediate with its correct value.
            if n >= 2 {
                let id = m.compile_forcing_bail(&case.func, n as u32).unwrap();
                let out = m.call_with_host_ctx(
                    id,
                    &case.args,
                    &case.lens,
                    0,
                    &mut [FlatBufferArg::Int(&ints)],
                );
                // reg 3 is the first arithmetic result in cases A and C; it is computed
                // at an earlier site yet must be captured at the final site.
                assert_eq!(
                    live_value(&out, 3),
                    Some((case.expect)(3)),
                    "{}: late site {} failed to capture earlier intermediate reg 3",
                    case.name,
                    n
                );
                late_intermediate_checks += 1;
            }
        }

        // Sanity: we actually exercised every site of every case plus late checks.
        assert_eq!(combinations, 2 + 1 + 3, "unexpected (case, site) coverage");
        assert!(late_intermediate_checks >= 2, "expected late-site checks");
    }

    #[test]
    fn compile_forcing_all_bails_deopts_at_first_executed_safepoint() {
        use JitValueType::{FlatInt, Int};

        let values = [10, 20, 30];
        let ptr = values.as_ptr() as i64;
        let func = ft(
            3,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 2,
                },
                JitInstr::Return { src: 4 },
            ],
        );

        let mut m = module();
        let id = m.compile_forcing_all_bails(&func).unwrap();
        assert_eq!(
            m.deopt_map(id).expect("map").sites.len(),
            2,
            "test function should have both add-overflow and direct-list guards",
        );
        let out = m.call_with_host_ctx(
            id,
            &[ptr, 7, 1],
            &[values.len() as i64, 0, 0],
            0,
            &mut [FlatBufferArg::Int(&values)],
        );
        match out {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(
                    live.iter().find(|reg| reg.reg == 1).map(|reg| reg.value),
                    Some(DeoptValue::Int(7))
                );
            }
            other => panic!("expected forced all-sites deopt, got {other:?}"),
        }
    }

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
        assert!(err.0.contains("out of range"), "{}", err.0);
    }

    #[test]
    fn rejects_out_of_range_jump_target() {
        let err = validate(&f(1, 1, vec![JitInstr::Jump { target: 9 }])).unwrap_err();
        assert!(err.0.contains("target 9"), "{}", err.0);
    }

    #[test]
    fn rejects_out_of_range_cold_block_hint() {
        let mut prog = f(1, 1, vec![JitInstr::Return { src: 0 }]);
        prog.cold_blocks.push(3);
        let err = validate(&prog).unwrap_err();
        assert!(err.0.contains("cold block instruction 3"), "{}", err.0);
    }

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
        assert!(err.0.contains("fall-through"), "{}", err.0);
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
        assert!(err.0.contains("reg_types length"), "{}", err.0);
    }

    #[test]
    fn rejects_params_exceeding_regs() {
        let err = validate(&f(4, 2, vec![])).unwrap_err();
        assert!(err.0.contains("n_params"), "{}", err.0);
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
        assert!(error.0.contains("analysis size"), "{}", error.0);
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
        assert!(err.0.contains("inconsistent result types"), "{}", err.0);
    }

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
        assert!(err.0.contains("CallSelf result"), "{}", err.0);
    }

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
        assert!(err.0.contains("flat-array parameters"), "{}", err.0);
    }

    #[test]
    fn rejects_reachable_use_before_definition() {
        let err = validate(&f(0, 1, vec![JitInstr::Return { src: 0 }]))
            .expect_err("undefined register reads must not become zero");
        assert!(
            err.0.contains("before it is definitely assigned"),
            "{}",
            err.0
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
                error.0.contains("before it is definitely assigned"),
                "{name}: {}",
                error.0
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
        assert!(error.0.contains("before it is definitely assigned"));
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
        assert!(err.0.contains("scalar type"), "{}", err.0);
    }

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
        assert!(err.0.contains("out of range"), "{}", err.0);

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
        assert!(err.0.contains("shared by instructions"), "{}", err.0);
    }

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
        assert!(err.0.contains("result must be a scalar"), "{}", err.0);
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
        assert!(err.0.contains("Int or Float"), "{}", err.0);

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
        assert!(err.0.contains("classes differ"), "{}", err.0);
    }

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
            assert!(err.0.contains("flat-array"), "{}", err.0);
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
        assert!(super::validate(&normal, false).is_err());

        let osr = f(1, 1, vec![JitInstr::Return { src: 0 }]);
        assert!(super::validate(&osr, true).is_err());
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
        assert!(err.0.contains("must be Int"), "{}", err.0);
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
        assert!(err.0.contains("classes differ"), "{}", err.0);
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
        assert!(err.0.contains("Handle"), "{}", err.0);
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
        assert!(err.0.contains("expected Handle"), "{}", err.0);
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
    fn two_param_add() -> JitFunction {
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
        assert!(err.0.contains("unsupported CallNative"), "{}", err.0);
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
            signal_bail();
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

    // --- J4.3: conservative range proof for eliding overflow checks -----------

    /// LoadInt c ⇒ [c, c]; an Add of two known constants whose sum fits i64 is
    /// proven non-overflowing (and the proof line is computed in i128).
    #[test]
    fn interval_load_and_add_constants() {
        let prog = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: 5 },
                JitInstr::LoadInt { dst: 1, value: 3 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // On entry to the Add (ip 2), reg0=[5,5], reg1=[3,3].
        assert_eq!(iv[2][0], Interval { lo: 5, hi: 5 });
        assert_eq!(iv[2][1], Interval { lo: 3, hi: 3 });
        // The Add is proven non-overflowing ([8,8] ⊂ i64).
        assert!(arith_cannot_overflow(&iv[2], &prog.code[2]));
        // The result [8,8] flows to the Return's in-set (ip 3).
        assert_eq!(iv[3][2], Interval { lo: 8, hi: 8 });
    }

    /// A proven-unchecked add of two large constants whose sum is STILL within i64
    /// produces the exact (non-wrapping) sum — the unchecked op is byte-identical to
    /// the checked one here because the proof guarantees no overflow.
    #[test]
    fn proven_large_constant_add_is_correct() {
        let mut m = module();
        // c = (i64::MAX - 10) + 10 = i64::MAX, proven safe ⇒ unchecked, exact.
        let id = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt {
                        dst: 0,
                        value: i64::MAX - 10,
                    },
                    JitInstr::LoadInt { dst: 1, value: 10 },
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[]), Some(i64::MAX));
    }

    /// Boundary: operand ranges summing to EXACTLY i64::MAX are proven safe; summing
    /// to i64::MAX + 1 are NOT (the analysis draws the line at the i64 boundary).
    #[test]
    fn boundary_exact_max_proven_overflow_unproven() {
        // Proven: (i64::MAX - 1) + 1 = i64::MAX, fits ⇒ unchecked.
        let safe = f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX - 1,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&safe);
        assert_eq!(
            iv[2][0],
            Interval {
                lo: i64::MAX as i128 - 1,
                hi: i64::MAX as i128 - 1
            }
        );
        assert!(arith_cannot_overflow(&iv[2], &safe.code[2]));

        // Just over: i64::MAX + 1 overflows ⇒ NOT proven, stays checked.
        let over = f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX,
                },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv2 = interval_analysis(&over);
        assert!(!arith_cannot_overflow(&iv2[2], &over.code[2]));
    }

    /// The proven-boundary add runs unchecked and yields exactly i64::MAX (no bail);
    /// the over-boundary constant add — which the proof leaves CHECKED — bails on its
    /// actual overflow. Same analysis, opposite emission, both correct.
    #[test]
    fn boundary_proven_runs_overflow_constant_bails() {
        let mut m = module();
        let safe = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt {
                        dst: 0,
                        value: i64::MAX - 1,
                    },
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
        assert_eq!(m.callt(safe, &[]), Some(i64::MAX));

        let over = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt {
                        dst: 0,
                        value: i64::MAX,
                    },
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
        // Constant operands are tracked, [i64::MAX]+[1] doesn't fit ⇒ stays checked ⇒
        // the sadd_overflow guard fires on the real overflow ⇒ Deopt (None).
        assert_eq!(m.callt(over, &[]), None);
    }

    /// Params are untracked (TOP). `a + b` over params stays CHECKED and bails on a
    /// real overflow (i64::MAX + 1) exactly as before — proving checks are NOT
    /// over-eagerly stripped.
    #[test]
    fn unknown_params_stay_checked_and_bail() {
        let mut m = module();
        // fn(a, b) -> Int { return a + b }, params untracked ⇒ TOP ⇒ checked.
        let prog = f(
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
        let iv = interval_analysis(&prog);
        // Both operands TOP ⇒ result range is TOP ⇒ not proven.
        assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
        let id = m.compile(&prog).unwrap();
        // In-range add returns the exact sum.
        assert_eq!(m.callt(id, &[2, 3]), Some(5));
        // i64::MAX + 1 overflows ⇒ the retained check bails.
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
    }

    #[test]
    fn unreachable_predecessors_cannot_narrow_entry_parameters() {
        let cases = [
            (
                "add",
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                [i64::MAX, 1],
            ),
            (
                "sub",
                JitInstr::Sub {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                [i64::MIN, 1],
            ),
            (
                "mul",
                JitInstr::Mul {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                [i64::MAX, 2],
            ),
        ];

        for (name, arithmetic, args) in cases {
            let program = f(
                2,
                3,
                vec![
                    arithmetic,
                    JitInstr::Return { src: 2 },
                    JitInstr::LoadInt { dst: 0, value: 0 },
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Jump { target: 0 },
                ],
            );
            let intervals = interval_analysis(&program);
            assert_eq!(intervals[0][0], Interval::TOP, "{name}");
            assert_eq!(intervals[0][1], Interval::TOP, "{name}");
            assert!(
                !arith_cannot_overflow(&intervals[0], &program.code[0]),
                "{name}"
            );

            let mut module = module();
            let id = module.compile(&program).expect(name);
            assert_eq!(module.callt(id, &args), None, "{name}");
        }
    }

    #[test]
    fn reachable_backedge_cannot_narrow_virtual_entry_parameters() {
        use JitValueType::{Bool, Int};

        let program = ft(
            3,
            vec![Int, Int, Bool, Int],
            vec![
                JitInstr::Add {
                    dst: 3,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::JumpIfBool {
                    cond: 2,
                    expected: true,
                    target: 5,
                },
                JitInstr::LoadInt { dst: 0, value: 0 },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Jump { target: 0 },
                JitInstr::Return { src: 3 },
            ],
        );
        let intervals = interval_analysis(&program);
        assert_eq!(intervals[0][0], Interval::TOP);
        assert_eq!(intervals[0][1], Interval::TOP);
        assert!(!arith_cannot_overflow(&intervals[0], &program.code[0]));

        let mut module = module();
        let id = module.compile(&program).expect("program should compile");
        assert_eq!(module.callt(id, &[i64::MAX, 1, 1]), None);
    }

    /// A register fed by `ListLen` is `[0, i64::MAX]` — non-negative but with NO
    /// tighter upper bound. So `len + len` can reach 2*i64::MAX, does NOT fit i64,
    /// and stays CHECKED (we did not assume a smaller length bound).
    #[test]
    fn list_len_is_nonneg_unbounded_above() {
        use JitValueType::{Handle, Int};
        let prog = ft(
            1,
            vec![Handle, Int, Int],
            vec![
                JitInstr::HostCall {
                    helper: HostHelper::ListLen,
                    dst: 1,
                    args: vec![HostArg::Reg(0)],
                },
                // len + len
                JitInstr::Add {
                    dst: 2,
                    lhs: 1,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // ListLen result is exactly [0, i64::MAX] on entry to the Add (ip 1).
        assert_eq!(
            iv[1][1],
            Interval {
                lo: 0,
                hi: i64::MAX as i128
            }
        );
        // [0,MAX]+[0,MAX] = [0, 2*MAX] does NOT fit i64 ⇒ stays checked.
        assert!(!arith_cannot_overflow(&iv[1], &prog.code[1]));
    }

    /// ListLenDirect is treated identically to ListLen: [0, i64::MAX].
    #[test]
    fn list_len_direct_is_nonneg() {
        use JitValueType::{FlatInt, Int};
        let prog = ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert_eq!(
            iv[1][1],
            Interval {
                lo: 0,
                hi: i64::MAX as i128
            }
        );
    }

    /// Sub and Mul interval transfer functions, plus their proven/unproven lines.
    #[test]
    fn interval_sub_and_mul_transfer() {
        // (10 - 3) = 7, proven; Move copies a range.
        let prog = f(
            0,
            4,
            vec![
                JitInstr::LoadInt { dst: 0, value: 10 },
                JitInstr::LoadInt { dst: 1, value: 3 },
                JitInstr::Sub {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Mul {
                    dst: 3,
                    lhs: 2,
                    rhs: 1,
                },
                JitInstr::Return { src: 3 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert!(arith_cannot_overflow(&iv[2], &prog.code[2])); // [10,10]-[3,3]=[7,7]
        assert_eq!(iv[3][2], Interval { lo: 7, hi: 7 });
        assert!(arith_cannot_overflow(&iv[3], &prog.code[3])); // [7,7]*[3,3]=[21,21]

        // A Mul of two large constants whose product overflows is NOT proven.
        let big = f(
            0,
            3,
            vec![
                JitInstr::LoadInt {
                    dst: 0,
                    value: i64::MAX,
                },
                JitInstr::LoadInt { dst: 1, value: 2 },
                JitInstr::Mul {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv2 = interval_analysis(&big);
        assert!(!arith_cannot_overflow(&iv2[2], &big.code[2]));
    }

    /// A register incremented across a loop back-edge widens to TOP (we infer no loop
    /// bound), so an unbounded accumulator's add stays CHECKED — and the fixpoint
    /// terminates.
    #[test]
    fn loop_accumulator_widens_to_top() {
        // i = 0; loop { i = i + 1; } (no exit) — i grows unbounded.
        // regs: 0=i, 1=one
        let prog = f(
            0,
            2,
            vec![
                JitInstr::LoadInt { dst: 0, value: 0 }, // 0
                JitInstr::LoadInt { dst: 1, value: 1 }, // 1
                JitInstr::Add {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                }, // 2: i = i + 1
                JitInstr::Jump { target: 2 },           // 3: back-edge to the Add
            ],
        );
        let iv = interval_analysis(&prog);
        // After widening, `i` on entry to the Add is TOP ⇒ the increment stays checked.
        assert_eq!(iv[2][0], Interval::TOP);
        assert!(!arith_cannot_overflow(&iv[2], &prog.code[2]));
    }

    // --- J4.3+: branch-conditioned range refinement for loop counters ----------

    /// Build the counted loop
    ///   fn f(limit){ i=0; total=0; while i<limit { total += step; i += incr }; total }
    /// in JIT IR. regs: 0=limit(param), 1=i, 2=total, 3=incr(const), 4=step(const 1).
    /// The guard is `JumpIfIntCompare i < limit, expected=false, target=exit`, so the
    /// FALL-THROUGH edge into the body asserts `i < limit` (the refinement site).
    fn counted_loop(incr: i64, op: JitCompare) -> JitFunction {
        f(
            1,
            5,
            vec![
                JitInstr::LoadInt { dst: 1, value: 0 }, // 0: i = 0
                JitInstr::LoadInt { dst: 2, value: 0 }, // 1: total = 0
                JitInstr::LoadInt {
                    dst: 3,
                    value: incr,
                }, // 2: incr
                JitInstr::LoadInt { dst: 4, value: 1 }, // 3: step = 1
                // 4: header guard — if !(i <op> limit) goto exit(8)
                JitInstr::JumpIfIntCompare {
                    lhs: 1,
                    rhs: 0,
                    op,
                    expected: false,
                    target: 8,
                },
                JitInstr::Add {
                    dst: 2,
                    lhs: 2,
                    rhs: 4,
                }, // 5: total = total + 1 (unbounded)
                JitInstr::Add {
                    dst: 1,
                    lhs: 1,
                    rhs: 3,
                }, // 6: i = i + incr (loop counter)
                JitInstr::Jump { target: 4 }, // 7: back-edge
                JitInstr::Return { src: 2 },  // 8: exit
            ],
        )
    }

    /// (i) Under the guard `i < limit`, the counter increment `i = i + 1` is proven
    /// UNCHECKED: on the loop-body edge `i <= limit - 1 <= i64::MAX - 1`, so
    /// `i + 1 <= i64::MAX` provably fits. The unbounded accumulator `total = total + 1`
    /// stays CHECKED (total widens to TOP — no bounding guard).
    #[test]
    fn loop_counter_lt_increment_proven_accumulator_checked() {
        let prog = counted_loop(1, JitCompare::Lt);
        let iv = interval_analysis(&prog);
        // Body in-set (ip 5/6): the refined `i` is bounded above by limit.hi - 1.
        // limit is TOP ([MIN, MAX]) ⇒ i.hi = MAX - 1.
        assert_eq!(iv[6][1].hi, i64::MAX as i128 - 1);
        // i = i + 1 (ip 6): [.., MAX-1] + [1,1] = [.., MAX] ⇒ fits ⇒ UNCHECKED.
        assert!(arith_cannot_overflow(&iv[6], &prog.code[6]));
        // total = total + 1 (ip 5): total is TOP ⇒ stays CHECKED.
        assert!(!arith_cannot_overflow(&iv[5], &prog.code[5]));
    }

    /// The same loop, compiled and run: the result equals the loop trip count for a
    /// large limit, confirming the UNCHECKED counter increment is correct at scale
    /// (no spurious bail, no wrong wrap). total stays small so its checked add is fine.
    #[test]
    fn loop_counter_lt_runs_correct_at_scale() {
        let mut m = module();
        let id = m.compile(&counted_loop(1, JitCompare::Lt)).unwrap();
        assert_eq!(m.callt(id, &[0]), Some(0));
        assert_eq!(m.callt(id, &[1]), Some(1));
        assert_eq!(m.callt(id, &[1_000_000]), Some(1_000_000));
    }

    /// (ii-a) `i = i + 2` must stay CHECKED: under `i < limit` we only know
    /// `i <= i64::MAX - 1`, so `i + 2` can reach `i64::MAX + 1` ⇒ does NOT fit.
    #[test]
    fn loop_counter_plus_two_stays_checked() {
        let prog = counted_loop(2, JitCompare::Lt);
        let iv = interval_analysis(&prog);
        // i still refined to [.., MAX-1], but [.., MAX-1] + [2,2] = [.., MAX+1] ⇒
        // does NOT fit i64 ⇒ CHECKED.
        assert_eq!(iv[6][1].hi, i64::MAX as i128 - 1);
        assert!(!arith_cannot_overflow(&iv[6], &prog.code[6]));
    }

    /// (ii-b) `while i <= limit` must keep `i = i + 1` CHECKED: the `Le` taken-edge
    /// only proves `i <= limit <= i64::MAX`, so `i` may BE `i64::MAX` and `i + 1`
    /// overflows. This locks the Lt-vs-Le off-by-one.
    #[test]
    fn loop_counter_le_increment_stays_checked() {
        let prog = counted_loop(1, JitCompare::Le);
        let iv = interval_analysis(&prog);
        // Under `i <= limit` (limit TOP), i.hi = min(MAX, limit.hi) = MAX, NOT MAX-1.
        assert_eq!(iv[6][1].hi, i64::MAX as i128);
        // [.., MAX] + [1,1] = [.., MAX+1] ⇒ does NOT fit ⇒ CHECKED.
        assert!(!arith_cannot_overflow(&iv[6], &prog.code[6]));
    }

    /// (iii) A bare `a + b` with unconstrained (TOP) operands and no governing guard
    /// stays CHECKED and bails on a real overflow — refinement never strips a check
    /// when there is no comparison fact to refine by. (Mirrors the param test, kept
    /// here so the J4.3+ slice asserts the negative directly.)
    #[test]
    fn unguarded_add_stays_checked_and_bails() {
        let mut m = module();
        let prog = f(
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
        let iv = interval_analysis(&prog);
        assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
        let id = m.compile(&prog).unwrap();
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None); // checked overflow ⇒ bail
    }

    /// The refinement is edge-SENSITIVE: the SAME register is bounded on the taken
    /// edge but TOP at the post-join loop header. A direct two-block check that the
    /// guard's taken (`<`) edge tightens lhs while the header stays TOP, and that an
    /// unreachable refinement (`x < x`) is handled soundly (no malformed interval).
    #[test]
    fn refinement_edge_sensitive_and_unreachable_safe() {
        // if i < limit { return i + 1 } else { return i }   regs 0=limit,1=i,2=t
        let prog = f(
            2,
            3,
            vec![
                // 0: if !(i < limit) goto 3 (else-branch)
                JitInstr::JumpIfIntCompare {
                    lhs: 1,
                    rhs: 0,
                    op: JitCompare::Lt,
                    expected: false,
                    target: 3,
                },
                JitInstr::Add {
                    dst: 2,
                    lhs: 1,
                    rhs: 1,
                }, // 1: t = i + i (on i<limit edge)
                JitInstr::Return { src: 2 }, // 2
                JitInstr::Return { src: 1 }, // 3: else
            ],
        );
        let iv = interval_analysis(&prog);
        // On the body edge i is refined to [MIN, limit.hi-1] = [MIN, MAX-1]; the
        // header in-set (ip 0) still sees i as TOP (param, untracked).
        assert_eq!(iv[0][1], Interval::TOP);
        assert_eq!(iv[1][1].hi, i64::MAX as i128 - 1);

        // Unreachable edge: `if i < i` — the taken edge asserts the empty fact i < i.
        // The two per-operand narrowings (i.hi <= i.hi-1, i.lo >= i.lo+1) each apply
        // to the SAME register but never invert it in a single `apply`, so the result
        // is a sound (over-approximating) but WELL-FORMED interval. The contract this
        // test locks is soundness's structural half: NO malformed interval (lo > hi)
        // ever escapes the refinement, even on an unreachable edge.
        let bad = f(
            1,
            2,
            vec![
                JitInstr::JumpIfIntCompare {
                    lhs: 0,
                    rhs: 0,
                    op: JitCompare::Lt,
                    expected: true,
                    target: 2,
                },
                JitInstr::Return { src: 0 }, // 1: fall-through (i >= i, always)
                JitInstr::Return { src: 0 }, // 2: taken (i < i, unreachable)
            ],
        );
        let iv2 = interval_analysis(&bad);
        // Every register interval at every ip is well-formed (lo <= hi).
        for row in &iv2 {
            for v in row {
                assert!(v.lo <= v.hi, "malformed interval {v:?}");
            }
        }
    }

    fn constant_mod_access(
        list_ty: JitValueType,
        result_ty: JitValueType,
        set_value: Option<(JitValueType, i64)>,
    ) -> JitFunction {
        let mut reg_types = vec![
            list_ty,
            JitValueType::Int,
            JitValueType::Int,
            JitValueType::Int,
            result_ty,
        ];
        let mut code = vec![
            JitInstr::LoadInt { dst: 1, value: 11 },
            JitInstr::LoadInt { dst: 2, value: 4 },
            JitInstr::Mod {
                dst: 3,
                lhs: 1,
                rhs: 2,
            },
        ];
        match (list_ty, set_value) {
            (JitValueType::FlatInt | JitValueType::FlatIntMut, None) => {
                code.push(JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                });
            }
            (JitValueType::FlatFloat | JitValueType::FlatFloatMut, None) => {
                code.push(JitInstr::ListGetFloatDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                });
            }
            (JitValueType::FlatIntMut, Some((value_ty, bits))) => {
                reg_types.push(value_ty);
                code.push(JitInstr::LoadInt {
                    dst: 5,
                    value: bits,
                });
                code.push(JitInstr::ListSetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                    value: 5,
                });
            }
            _ => unreachable!("unsupported test list operation"),
        }
        code.push(JitInstr::Return { src: 4 });
        ft(1, reg_types, code)
    }

    #[test]
    fn mod_interval_transfer_tracks_result_sign_and_magnitude() {
        let positive = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: 17 },
                JitInstr::LoadInt { dst: 1, value: 5 },
                JitInstr::Mod {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        let positive_intervals = interval_analysis(&positive);
        assert_eq!(positive_intervals[3][2], Interval { lo: 0, hi: 4 });

        let negative = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: -17 },
                JitInstr::LoadInt { dst: 1, value: 5 },
                JitInstr::Mod {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        assert_eq!(
            interval_analysis(&negative)[3][2],
            Interval { lo: -4, hi: 0 }
        );
    }

    #[test]
    fn list_bounds_plan_accepts_only_unique_sound_provenance() {
        use JitValueType::{FlatInt, Int};

        let constant = constant_mod_access(FlatInt, Int, None);
        let constant_plan = list_bounds_plan(&constant, &interval_analysis(&constant), false);
        assert_eq!(constant_plan.unchecked_ips, [3].into_iter().collect());
        assert_eq!(constant_plan.entry_min_len.get(&0), Some(&4));

        // Unique Move forwarding is provenance-preserving for both divisor and
        // modulo result.
        let moved = ft(
            1,
            vec![FlatInt, Int, Int, Int, Int, Int, Int],
            vec![
                JitInstr::LoadInt { dst: 1, value: 9 },
                JitInstr::LoadInt { dst: 2, value: 3 },
                JitInstr::Move { dst: 3, src: 2 },
                JitInstr::Mod {
                    dst: 4,
                    lhs: 1,
                    rhs: 3,
                },
                JitInstr::Move { dst: 5, src: 4 },
                JitInstr::ListGetIntDirect {
                    dst: 6,
                    base: 0,
                    index: 5,
                },
                JitInstr::Return { src: 6 },
            ],
        );
        assert!(
            list_bounds_plan(&moved, &interval_analysis(&moved), false)
                .unchecked_ips
                .contains(&5)
        );

        // A second reachable definition makes the divisor ambiguous even though
        // the last definition is also positive.
        let multi_def = ft(
            1,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::LoadInt { dst: 1, value: 9 },
                JitInstr::LoadInt { dst: 2, value: 3 },
                JitInstr::LoadInt { dst: 2, value: 4 },
                JitInstr::Mod {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                },
                JitInstr::Return { src: 4 },
            ],
        );
        assert!(
            list_bounds_plan(&multi_def, &interval_analysis(&multi_def), false)
                .unchecked_ips
                .is_empty()
        );

        // A parameter's incoming value is its first definition. Overwriting it
        // once in code is still multi-def and cannot manufacture provenance.
        let overwritten_param = ft(
            3,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::LoadInt { dst: 1, value: 9 },
                JitInstr::LoadInt { dst: 2, value: 4 },
                JitInstr::Mod {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                },
                JitInstr::Return { src: 4 },
            ],
        );
        assert!(
            list_bounds_plan(
                &overwritten_param,
                &interval_analysis(&overwritten_param),
                false,
            )
            .unchecked_ips
            .is_empty()
        );
    }

    #[test]
    fn constant_modulo_elides_all_direct_get_and_set_checks() {
        use JitValueType::{FlatFloat, FlatFloatMut, FlatInt, FlatIntMut, Float, Int};
        let mut m = module();

        let int_get = m.compile(&constant_mod_access(FlatInt, Int, None)).unwrap();
        let ints = [10, 20, 30, 40];
        assert_eq!(
            m.call_with_host_ctx(
                int_get,
                &[ints.as_ptr() as i64],
                &[ints.len() as i64],
                0,
                &mut [FlatBufferArg::Int(&ints)],
            )
            .completed(),
            Some(40)
        );

        let float_get = m
            .compile(&constant_mod_access(FlatFloat, Float, None))
            .unwrap();
        let floats = [1.0, 2.0, 3.0, 4.5];
        let bits = m
            .call_with_host_ctx(
                float_get,
                &[floats.as_ptr() as i64],
                &[floats.len() as i64],
                0,
                &mut [FlatBufferArg::Float(&floats)],
            )
            .completed()
            .unwrap();
        assert_eq!(f64::from_bits(bits as u64), 4.5);

        let int_set = m
            .compile(&constant_mod_access(FlatIntMut, Int, Some((Int, 99))))
            .unwrap();
        let mut mutable = [10, 20, 30, 40];
        let mutable_ptr = mutable.as_mut_ptr() as i64;
        assert_eq!(
            m.call_with_host_ctx(
                int_set,
                &[mutable_ptr],
                &[mutable.len() as i64],
                0,
                &mut [FlatBufferArg::IntMut(&mut mutable)],
            )
            .completed(),
            Some(0)
        );
        assert_eq!(mutable, [10, 20, 30, 99]);

        let float_set_program = ft(
            1,
            vec![FlatFloatMut, Int, Int, Int, Float, Int],
            vec![
                JitInstr::LoadInt { dst: 1, value: 11 },
                JitInstr::LoadInt { dst: 2, value: 4 },
                JitInstr::Mod {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::LoadFloat {
                    dst: 4,
                    value: 9.25,
                },
                JitInstr::ListSetFloatDirect {
                    dst: 5,
                    base: 0,
                    index: 3,
                    value: 4,
                },
                JitInstr::Return { src: 5 },
            ],
        );
        assert!(
            list_bounds_plan(
                &float_set_program,
                &interval_analysis(&float_set_program),
                false,
            )
            .unchecked_ips
            .contains(&4)
        );
        let float_set = m.compile(&float_set_program).unwrap();
        let mut mutable_floats = [1.0, 2.0, 3.0, 4.0];
        let mutable_floats_ptr = mutable_floats.as_mut_ptr() as i64;
        assert_eq!(
            m.call_with_host_ctx(
                float_set,
                &[mutable_floats_ptr],
                &[mutable_floats.len() as i64],
                0,
                &mut [FlatBufferArg::FloatMut(&mut mutable_floats)],
            )
            .completed(),
            Some(0)
        );
        assert_eq!(mutable_floats, [1.0, 2.0, 3.0, 9.25]);
    }

    #[test]
    fn constant_modulo_short_list_deopts_anonymously_before_source() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        let program = constant_mod_access(FlatInt, Int, None);
        let id = m.compile(&program).unwrap();
        // Only the two checked-Mod sites remain. The direct access at ip 3 has no
        // safepoint, while its entry length guard is intentionally anonymous.
        let map = m.deopt_map(id).unwrap();
        assert_eq!(map.sites.len(), 2);
        assert!(map.sites.iter().all(|site| site.resume_ip == 2));

        let short = [10, 20, 30];
        assert!(matches!(
            m.call_with_host_ctx(
                id,
                &[short.as_ptr() as i64],
                &[short.len() as i64],
                0,
                &mut [FlatBufferArg::Int(&short)],
            ),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live,
                ..
            } if live.is_empty()
        ));
    }

    #[test]
    fn same_base_list_len_modulo_elides_access_but_preserves_mod_deopt_ip() {
        use JitValueType::{FlatInt, Int};
        let program = ft(
            1,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::LoadInt { dst: 2, value: 7 },
                JitInstr::Mod {
                    dst: 3,
                    lhs: 2,
                    rhs: 1,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                },
                JitInstr::Return { src: 4 },
            ],
        );
        let plan = list_bounds_plan(&program, &interval_analysis(&program), false);
        assert_eq!(plan.unchecked_ips, [3].into_iter().collect());
        assert!(plan.entry_min_len.is_empty());

        let mut m = module();
        let id = m.compile(&program).unwrap();
        assert_eq!(m.deopt_map(id).unwrap().sites.len(), 2);
        let empty: [i64; 0] = [];
        match m.call_with_host_ctx(
            id,
            &[empty.as_ptr() as i64],
            &[0],
            0,
            &mut [FlatBufferArg::Int(&empty)],
        ) {
            NativeOutcome::Deopt {
                safepoint_id, live, ..
            } => {
                assert_eq!(safepoint_id, SafepointId(1));
                assert_eq!(m.deopt_map(id).unwrap().sites[0].resume_ip, 2);
                assert!(live.iter().any(|value| value.reg == 1));
            }
            outcome => panic!("expected modulo deopt, got {outcome:?}"),
        }
    }

    #[test]
    fn negative_and_wrong_base_modulo_accesses_stay_checked() {
        use JitValueType::{FlatInt, Int};
        let negative = ft(
            1,
            vec![FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::LoadInt { dst: 1, value: -1 },
                JitInstr::LoadInt { dst: 2, value: 4 },
                JitInstr::Mod {
                    dst: 3,
                    lhs: 1,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 4,
                    base: 0,
                    index: 3,
                },
                JitInstr::Return { src: 4 },
            ],
        );
        assert!(
            list_bounds_plan(&negative, &interval_analysis(&negative), false)
                .unchecked_ips
                .is_empty()
        );

        let wrong_base = ft(
            2,
            vec![FlatInt, FlatInt, Int, Int, Int, Int],
            vec![
                JitInstr::ListLenDirect { dst: 2, base: 0 },
                JitInstr::LoadInt { dst: 3, value: 7 },
                JitInstr::Mod {
                    dst: 4,
                    lhs: 3,
                    rhs: 2,
                },
                JitInstr::ListGetIntDirect {
                    dst: 5,
                    base: 1,
                    index: 4,
                },
                JitInstr::Return { src: 5 },
            ],
        );
        assert!(
            list_bounds_plan(&wrong_base, &interval_analysis(&wrong_base), false)
                .unchecked_ips
                .is_empty()
        );

        let mut m = module();
        let negative_id = m.compile(&negative).unwrap();
        let values = [10, 20, 30, 40];
        match m.call_with_host_ctx(
            negative_id,
            &[values.as_ptr() as i64],
            &[values.len() as i64],
            0,
            &mut [FlatBufferArg::Int(&values)],
        ) {
            NativeOutcome::Deopt { safepoint_id, .. } => {
                let site = &m.deopt_map(negative_id).unwrap().sites[safepoint_id.0 as usize - 1];
                assert_eq!(site.resume_ip, 3);
            }
            outcome => panic!("expected checked negative-index deopt, got {outcome:?}"),
        }

        let wrong_base_id = m.compile(&wrong_base).unwrap();
        let divisor_list = [10, 20, 30, 40];
        let target_list = [99];
        match m.call_with_host_ctx(
            wrong_base_id,
            &[divisor_list.as_ptr() as i64, target_list.as_ptr() as i64],
            &[divisor_list.len() as i64, target_list.len() as i64],
            0,
            &mut [
                FlatBufferArg::Int(&divisor_list),
                FlatBufferArg::Int(&target_list),
            ],
        ) {
            NativeOutcome::Deopt { safepoint_id, .. } => {
                let site = &m.deopt_map(wrong_base_id).unwrap().sites[safepoint_id.0 as usize - 1];
                assert_eq!(site.resume_ip, 3);
            }
            outcome => panic!("expected checked wrong-base deopt, got {outcome:?}"),
        }
    }
    #[test]
    fn validated_compile_boundary_rejects_malformed_ir_before_codegen() {
        let malformed = f(0, 1, vec![JitInstr::Jump { target: 9 }]);
        let error = match validate_function(&malformed) {
            Ok(_) => panic!("malformed IR must not mint a validation proof"),
            Err(error) => error,
        };
        assert!(error.0.contains("target 9"));

        let mut module = module();
        let error = module
            .compile(&malformed)
            .expect_err("the compatibility entry must validate before codegen");
        assert!(error.0.contains("target 9"));
        assert_eq!(module.funcs.len(), 0);
    }

    #[test]
    fn validated_compile_boundary_accepts_a_sealed_proof() {
        let function = f(
            0,
            1,
            vec![
                JitInstr::LoadInt { dst: 0, value: 7 },
                JitInstr::Return { src: 0 },
            ],
        );
        let validated = validate_function(&function).expect("valid IR");
        let mut module = module();
        let id = module
            .compile_validated(&validated)
            .expect("sealed IR reaches codegen");
        assert_eq!(module.call(id, &[], &[]).completed(), Some(7));
    }

    #[test]
    fn architecture_keeps_validation_and_codegen_in_separate_files() {
        let root = include_str!("lib.rs");
        let module = include_str!("module.rs");
        let validation = include_str!("ir_validation.rs");
        let codegen = include_str!("codegen.rs");

        assert!(root.contains("include!(\"ir_validation.rs\")"));
        assert!(root.contains("include!(\"codegen.rs\")"));
        assert!(module.contains("include!(\"deopt.rs\")"));
        assert!(module.contains("validated: &ValidatedJitFunction<'_>"));
        assert!(!module.contains("fn validate(program: &JitFunction"));
        assert!(validation.contains("fn validate(program: &JitFunction"));
        assert!(codegen.contains("fn build_function("));
        assert!(!codegen.contains("pub fn build_function("));
    }
}
