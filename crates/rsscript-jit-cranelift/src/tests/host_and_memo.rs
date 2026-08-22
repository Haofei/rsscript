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
        err.message
            .contains("heap-writing helpers cannot be memoized"),
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
    assert!(
        error.message.contains("does not belong"),
        "{}",
        error.message
    );

    let mut interior_entry = nested_memo_program(false);
    interior_entry.code[0] = JitInstr::Jump { target: 5 };
    let error =
        validate(&interior_entry).expect_err("outside control flow cannot enter the interior");
    assert!(
        error.message.contains("enters scope interior"),
        "{}",
        error.message
    );

    let mut conditional_backedge = nested_memo_program(false);
    conditional_backedge.code[9] = JitInstr::JumpIfIntCompare {
        lhs: 6,
        rhs: 3,
        op: JitCompare::Lt,
        expected: true,
        target: 4,
    };
    let error = validate(&conditional_backedge).expect_err("conditional backedges are unsupported");
    assert!(
        error.message.contains("must be an unconditional Jump"),
        "{}",
        error.message
    );

    let mut bad_range = nested_memo_program(false);
    bad_range.memo_scopes[0].exit = bad_range.code.len() as u32;
    let error = validate(&bad_range).expect_err("scope exit must name an instruction");
    assert!(
        error.message.contains("header < exit < code length"),
        "{}",
        error.message
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

static MAP_GET_MATCH_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static MAP_GET_MATCH_FLOAT_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static SORTED_MAP_GET_INT_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static SORTED_MAP_GET_FLOAT_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static MAP_CONTAINS_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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
    signal_bail(_ctx);
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
use super::*;
