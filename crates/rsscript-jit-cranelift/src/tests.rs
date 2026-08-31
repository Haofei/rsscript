use super::*;

#[test]
fn compilation_phase_timings_separate_validation_codegen_and_finalize() {
    let mut module = module();
    let function = validation::two_param_add();
    let proof = module.validate_region(&function).expect("IR validates");
    let after_validation = module.compile_phase_timings();
    assert!(after_validation.validation_nanos > 0);
    assert_eq!(after_validation.codegen_nanos, 0);
    assert_eq!(after_validation.finalize_nanos, 0);

    module
        .compile_validated(&proof)
        .expect("validated IR compiles");
    let published = module.compile_phase_timings();
    assert_eq!(
        published.validation_nanos,
        after_validation.validation_nanos
    );
    assert!(published.codegen_nanos > 0);
    assert!(published.finalize_nanos > 0);
}

#[test]
fn map_match_effect_metadata_covers_int_and_float_symmetrically() {
    let instructions = [
        JitInstr::MatchMapGetInt {
            map: 1,
            key: 2,
            value_dst: 3,
            some_ip: 4,
            none_ip: 5,
        },
        JitInstr::MatchMapGetFloat {
            map: 1,
            key: 2,
            value_dst: 3,
            some_ip: 4,
            none_ip: 5,
        },
        JitInstr::MatchSortedMapGetInt {
            map: 1,
            key: 2,
            value_dst: 3,
            some_ip: 4,
            none_ip: 5,
        },
        JitInstr::MatchSortedMapGetFloat {
            map: 1,
            key: 2,
            value_dst: 3,
            some_ip: 4,
            none_ip: 5,
        },
    ];
    for instruction in instructions {
        let descriptor = instruction.descriptor();
        assert_eq!(instruction.defined_register(), Some(3));
        let mut uses = Vec::new();
        instruction.visit_used_registers(|reg| uses.push(reg));
        assert_eq!(uses, [1, 2]);
        assert_eq!(
            instruction.effects(),
            JitInstrEffects {
                control_flow: JitControlFlow::Split {
                    first: 4,
                    second: 5,
                },
                heap: JitHeapEffect::Read,
                may_deopt: true,
                osr_supported: true,
            }
        );
        let mut heap_inputs = Vec::new();
        instruction.visit_osr_heap_inputs(|reg| heap_inputs.push(reg));
        assert_eq!(heap_inputs, [1]);
        assert_eq!(descriptor.effects, instruction.effects());
        assert!(descriptor.required_host_helper.is_some());
        assert!(!descriptor.flat_list_direct);
    }
}

#[test]
fn instruction_descriptor_owns_flat_and_helper_capabilities() {
    let direct = JitInstr::ListGetIntDirect {
        dst: 0,
        base: 1,
        index: 2,
    };
    let direct_descriptor = direct.descriptor();
    assert!(direct_descriptor.flat_list_direct);
    assert_eq!(direct_descriptor.cost_class, JitInstrCostClass::DirectList);
    assert!(direct_descriptor.required_host_helper.is_none());

    let host = JitInstr::HostCall {
        dst: 0,
        helper: HostHelper::StringLen,
        args: vec![HostArg::Reg(1)],
    };
    let host_descriptor = host.descriptor();
    assert_eq!(
        host_descriptor.required_host_helper,
        Some(HostHelper::StringLen)
    );
    assert_eq!(host_descriptor.cost_class, JitInstrCostClass::HostCall);
    assert!(!host_descriptor.compact_scalar_frame);
}

/// Test shim: validate as a non-OSR program (the common case for these IR
/// validation tests). OSR-specific validation is exercised via `compile_osr`.
fn validate(program: &JitFunction) -> Result<(), JitError> {
    super::validate(program, false)
}

#[test]
fn call_frame_layout_is_versioned_and_stable() {
    assert_eq!(JIT_CALL_ABI_VERSION, 3);
    assert_eq!(
        CALL_FRAME_SIZE as usize,
        std::mem::size_of::<JitCallFrame>()
    );
    assert_eq!(
        FRAME_ARGS as usize,
        std::mem::offset_of!(JitCallFrame, args)
    );
    assert_eq!(
        FRAME_DEOPT as usize,
        std::mem::offset_of!(JitCallFrame, deopt)
    );
    assert_eq!(
        FRAME_LOGICAL_DEPTH_LIMIT as usize,
        std::mem::offset_of!(JitCallFrame, logical_depth_limit)
    );
    #[cfg(target_pointer_width = "64")]
    assert_eq!(CALL_FRAME_SIZE, 112);
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
    assert!(error.message.contains("private found output"), "{error:?}");
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
        decline: None,
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
extern "C" fn noop_field_set_float(_ctx: HostCtx, _handle: i64, _slot: i64, _value: f64) -> i64 {
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
extern "C" fn noop_list_set_float(_ctx: HostCtx, _handle: i64, _index: i64, _value: f64) -> i64 {
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
extern "C" fn noop_string_pad_left_len(_ctx: HostCtx, _value: i64, _width: i64, _fill: i64) -> i64 {
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
extern "C" fn noop_map_get_match_int(_ctx: HostCtx, _map: i64, _key: i64, found: &mut i64) -> i64 {
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
extern "C" fn noop_sorted_map_insert_int(_ctx: HostCtx, _map: i64, _key: i64, _value: i64) -> i64 {
    0
}
extern "C" fn noop_sorted_map_get_int(_ctx: HostCtx, _map: i64, _key: i64, found: &mut i64) -> i64 {
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
extern "C" fn noop_deadline_expired(_ctx: HostCtx) -> i64 {
    0
}

fn host_helpers() -> HostHelpers {
    HostHelpers {
        deadline_expired: noop_deadline_expired,
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
        instruction_origins: Vec::new(),
        source_instruction_count: 0,
        memo_scopes: Vec::new(),
        cold_blocks: Vec::new(),
        resume_live_regs: Vec::new(),
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
        instruction_origins: Vec::new(),
        source_instruction_count: 0,
        memo_scopes: Vec::new(),
        cold_blocks: Vec::new(),
        resume_live_regs: Vec::new(),
    }
}

mod calls_and_abi;
mod deopt;
mod fuzz;
mod host_and_memo;
mod ranges;
mod validated_boundary;
mod validation;
