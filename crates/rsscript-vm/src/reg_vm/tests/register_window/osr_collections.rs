    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_finds_later_native_loop_after_setup_loop() {
        let source = r#"

fn bench_size(args: read List<String>, default: Int) -> Int {
    let raw = Arguments.get_or_default(args: read args, index: 0, default: read String.from_int(value: default))
    match String.parse_int(value: read raw) {
        Some(value) => {
            return value
        }
        None => {
            return default
        }
    }
}

fn main(args: read List<String>) -> Unit {
    let limit = bench_size(args: read args, default: 40000)
    let mut index = 0
    local values = List<Int>.new()

    while index < limit {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < List.len<Int>(list: read values) {
        total = total + List.get<Int>(list: read values, index: index)
        index = index + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = crate::hir::Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed");
        let func = unit.functions[unit.function_ids["main"]].as_ref();

        assert!(
            detect_single_natural_loop(&func.code).is_none(),
            "legacy single-loop detector should reject a setup loop plus hot loop",
        );
        let loops = detect_natural_loops(&func.code);
        assert!(
            loops.len() >= 2,
            "multi-loop detector should find both setup and read loops: {loops:?}",
        );
        let native_candidate = super::super::tier::select_osr_candidate_loop(&unit, func)
            .unwrap_or_else(|| {
                panic!(
                    "read-only list loop should be raw native-subset; loops={loops:?}; code={:#?}",
                    func.code
                )
            });
        assert!(
            native_candidate.header > loops[0].header,
            "candidate should be the later read loop, not the List.push setup loop",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_accepts_nonparam_list_push_growth_loop() {
        // J0.4 #1: a growth loop on a function-LOCAL (non-parameter) list is now a valid
        // OSR candidate. The local list is handle-accessed (flat-buffer pinning is
        // params-only), so growing it via the journaled `ListPushInt` helper is safe —
        // unlike a flat PARAM buffer, which would dangle on realloc and stays vetoed.
        let source = r#"

fn main(args: read List<String>) -> Unit {
    let limit = 64
    let mut index = 0
    local values = List<Int>.new()

    while index < limit {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    Output.write(message: read String.from_int(value: List.len<Int>(list: read values)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        assert!(
            super::super::tier::select_osr_candidate_loop(&executable.unit, func).is_some(),
            "a non-parameter (handle-accessed) list growth loop should be OSR-eligible; code={:#?}",
            func.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_later_native_loop_after_setup_loop() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    let limit = 10
    let mut index = 0
    local values = List<Int>.new()

    while index < limit {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    index = 0
    let mut total = 0
    while index < List.len<Int>(list: read values) {
        total = total + List.get<Int>(list: read values, index: index)
        index = index + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("compiled executable should expose a raw native loop");
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected loop should translate to OSR native IR: {candidate:?}; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let expected_header = candidate.header;
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            std::iter::empty::<(String, ExternalFunction)>().collect(),
        );
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        assert_eq!(
            vm.resolve_osr_candidate(func),
            Some(expected_header),
            "resolver should select the later raw native loop",
        );
        let (_out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "later raw native-subset loop should OSR-enter; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_two_sequential_regions() {
        let source = r#"
fn main(args: read List<String>) -> Unit {
    let mut first = 0
    let mut total = 0
    while first < 80 {
        total = total + first * 3 + first / 2
        first = first + 1
    }

    let mut second = 0
    while second < 90 {
        total = total + second * 5 - second / 3
        second = second + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("osr-sequential-regions.rss", source).expect("compile");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        let candidates = super::super::tier::select_osr_candidate_loops(&executable.unit, func);
        assert_eq!(
            candidates.len(),
            2,
            "both sequential loops should be bounded OSR candidates: {candidates:?}"
        );
        assert_ne!(candidates[0].header, candidates[1].header);

        let reference = executable
            .eval_main_with_args(std::iter::empty::<&str>())
            .expect("interpreter run");
        let (native, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("forced OSR run");
        assert_eval_observables_eq(&native, &reference);
        assert_eq!(
            stats.osr_entries, 2,
            "each sequential loop should enter its distinct region once: {stats:?}"
        );
        assert_eq!(
            stats.compiled, 2,
            "each sequential loop should compile a distinct RegionKey: {stats:?}"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_outer_decline_does_not_block_nested_inner_region() {
        let source = r#"
fn main(args: read List<String>) -> Unit {
    let adjust: Fn(Int) -> Int = |value| { return value + 7 }
    let mut outer = 0
    let mut total = 0
    while outer < 2 {
        let mut inner = 0
        while inner < 80 {
            total = total + inner * 3 - inner / 2
            inner = inner + 1
        }
        total = total + adjust(outer)
        outer = outer + 1
    }
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable = reg_vm_compile_source("osr-nested-fallback.rss", source).expect("compile");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        let candidates = super::super::tier::select_osr_candidate_loops(&executable.unit, func);
        assert_eq!(
            candidates.len(),
            2,
            "outer and inner loops should both be considered: {candidates:?}"
        );
        let outer = *candidates
            .iter()
            .find(|lp| {
                func.code[lp.header..lp.exit]
                    .iter()
                    .any(|instr| matches!(instr, RegInstr::CallClosure { .. }))
            })
            .expect("outer candidate should contain the cold closure call");
        let inner = *candidates
            .iter()
            .find(|lp| lp.header != outer.header)
            .expect("inner candidate");
        assert!(
            translate_osr_loop(&func.code, func.regs, func.params, func.captures, outer).is_none(),
            "cold outer closure region should decline direct OSR translation"
        );
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, inner)
            .expect("nested scalar loop should translate");

        let reference = executable
            .eval_main_with_args(std::iter::empty::<&str>())
            .expect("interpreter run");
        let (native, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("forced OSR run");
        assert_eval_observables_eq(&native, &reference);
        assert_eq!(
            stats.osr_entries, 2,
            "the inner region should enter once per outer iteration: {stats:?}"
        );
        assert_eq!(
            stats.compiled - stats.translated,
            1,
            "only the inner RegionKey should compile after the outer decline: {stats:?}"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_direct_async_call_await_loop() {
        let source = r#"

async fn step(value: Int) -> Result<Int, String> {
    return Ok(value + 1)
}

async fn main(args: read List<String>) -> Result<Unit, String> {
    let mut index = 0
    let mut total = 0
    while index < 2000 {
        total = await step(value: total)?
        index = index + 1
    }
    Output.write(message: read String.from_int(value: total))
    return Ok(Unit)
}
"#;
        let executable =
            reg_vm_compile_source("async_call_loop.rss", source).expect("lowering should succeed");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .unwrap_or_else(|| {
                panic!(
                    "direct async call/await loop should be selected for OSR; main code={:#?}",
                    func.code
                )
            });
        let (_out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("direct async call/await loop should run");
        assert!(
            stats.osr_entries > 0,
            "direct async call/await loop should OSR-enter; candidate={candidate:?}; stats={stats:?}; region={:#?}",
            &func.code[candidate.header..candidate.exit],
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_direct_async_option_call_await_loop() {
        let source = r#"

async fn step(value: Int) -> Option<Int> {
    return Some(value + 1)
}

async fn main(args: read List<String>) -> Option<Unit> {
    let mut index = 0
    let mut total = 0
    while index < 2000 {
        total = await step(value: total)?
        index = index + 1
    }
    Output.write(message: read String.from_int(value: total))
    return Some(Unit)
}
"#;
        let executable = reg_vm_compile_source("async_option_call_loop.rss", source)
            .expect("lowering should succeed");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .unwrap_or_else(|| {
                panic!(
                    "direct async Option call/await loop should be selected for OSR; main code={:#?}",
                    func.code
                )
            });
        let (_out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("direct async Option call/await loop should run");
        assert!(
            stats.osr_entries > 0,
            "direct async Option call/await loop should OSR-enter; candidate={candidate:?}; stats={stats:?}; region={:#?}",
            &func.code[candidate.header..candidate.exit],
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_task_group_spawn_loop() {
        let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/vm-jit/kernels/task_group_spawn.rss");
        let source =
            std::fs::read_to_string(source_path).expect("task-group benchmark source should exist");
        let executable = reg_vm_compile_source("task_group_spawn.rss", &source)
            .expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .unwrap_or_else(|| {
                panic!(
                    "task_group loop should be selected for OSR after pure spawn/join inlining; main code={:#?}",
                    func.code
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(["2000"])
            .expect("task-group benchmark should run");

        assert_eq!(out.stdout, "12012000\n");
        assert!(
            stats.osr_entries > 0,
            "task_group spawn/join loop should OSR-enter; candidate={candidate:?}; stats={stats:?}; region={:#?}",
            &func.code[candidate.header..candidate.exit],
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_selects_and_lowers_readonly_full_list_slice_loop() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    let limit = 10
    local base = List<Int>.new()
    let mut k = 0
    while k < 8 {
        let v = k * k - k
        List.push<Int>(list: mut base, value: read v)
        k = k + 1
    }
    let n = List.len<Int>(list: read base)

    let mut index = 0
    let mut total = 0
    while index < limit {
        local copy = List.slice(list: read base, start: 0, len: n)
        total = total + List.get<Int>(list: read copy, index: 0)
        index = index + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let func = executable.unit.functions[executable.unit.function_ids["main"]].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .unwrap_or_else(|| {
                panic!(
                    "full-slice read loop should be selected for OSR; code={:#?}",
                    func.code
                )
            });
        let loops = detect_natural_loops(&func.code);
        let loop_regions: Vec<_> = loops
            .iter()
            .map(|lp| (*lp, &func.code[lp.header..lp.exit]))
            .collect();

        assert!(
            func.code[candidate.header..candidate.exit]
                .iter()
                .any(|instr| matches!(
                    instr,
                    RegInstr::CallIntrinsic {
                        intrinsic: RegIntrinsic::ListSlice,
                        ..
                    }
                )),
            "selected loop should be the copy/read loop before the pre-eligibility rewrite; candidate={candidate:?}; loops={:?}; region={:#?}",
            loop_regions,
            &func.code[candidate.header..candidate.exit],
        );

        let (code, n_regs, _) = native_elide_readonly_full_list_slices_in_region(
            &func.code,
            func.regs,
            candidate.header,
            candidate.exit,
        )
        .expect("full read-only slice loop should rewrite");
        let lp = detect_natural_loop_at(&code, candidate.header)
            .expect("loop should remain after full-slice elision");

        translate_osr_loop(&code, n_regs, func.params, func.captures, lp).unwrap_or_else(|| {
            panic!(
                "full-slice-elided loop should translate to native OSR; region={:#?}",
                &code[lp.header..lp.exit],
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_transactional_list_set_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local values = List<Int>.new()
    let mut i = 0
    while i < 8 {
        List.push<Int>(list: mut values, value: read i)
        i = i + 1
    }

    i = 0
    let mut total = 0
    while i < 32 {
        let slot = i % 8
        List.set<Int>(list: mut values, index: slot, value: read i)
        total = total + List.get<Int>(list: read values, index: slot)
        i = i + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let (_out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "List.set<Int> loop should OSR-enter via transactional helper; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_flat_int_list_direct_write_commits_to_vm_list() {
        let source = r#"

fn hot(values: mut List<Int>, slot: Int, replacement: Int) -> Int {
    List.set<Int>(list: mut values, index: slot, value: read replacement)
    return List.get<Int>(list: read values, index: slot)
}

fn main(args: read List<String>) -> Unit {
    local values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)

    let total = hot(values: mut values, slot: 1, replacement: 7) + List.get<Int>(list: read values, index: 1)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");

        assert_eq!(
            out.stdout.trim(),
            "14",
            "native direct list write must commit before interpreter reads the list again"
        );
        assert!(
            stats.native_calls > 0,
            "hot list-write function should run through whole-function native; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_direct_flat_int_list_write_with_cold_push_trap() {
        let source = r#"

fn hot(values: mut List<Int>, limit: Int) -> Int {
    let mut i = 0
    let mut total = 0

    while i < limit {
        let slot = i % 8
        if i < 0 {
            List.push<Int>(list: mut values, value: read i)
        }
        List.set<Int>(list: mut values, index: slot, value: read i)
        total = total + List.get<Int>(list: read values, index: slot)
        i = i + 1
    }

    return total
}

fn main(args: read List<String>) -> Unit {
    local values = List<Int>.new()
    let mut i = 0
    while i < 8 {
        List.push<Int>(list: mut values, value: read 0)
        i = i + 1
    }

    let total = hot(values: mut values, limit: 32) + List.get<Int>(list: read values, index: 7)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["hot"];
        let hot_func = executable.unit.functions[hot].as_ref();
        let candidate = detect_single_natural_loop(&hot_func.code)
            .expect("hot list loop should be a single natural loop");
        let (jit, _, _, _, _, _, _) = translate_osr_loop(
            &hot_func.code,
            hot_func.regs,
            hot_func.params,
            hot_func.captures,
            candidate,
        )
        .unwrap_or_else(|| {
            panic!(
                "OSR should direct-access steady-state List.set/get and keep push as a cold trap; region={:#?}",
                &hot_func.code[candidate.header..candidate.exit],
            )
        });
        assert!(
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListSetIntDirect { .. })),
            "steady-state List.set should lower to direct flat write; jit={:#?}",
            jit.code,
        );
        assert!(
            !jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListGetIntDirect { .. })),
            "the adjacent steady-state List.get should forward the stored value; jit={:#?}",
            jit.code,
        );

        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert_eq!(out.stdout.trim(), "527");
        assert!(
            stats.osr_entries > 0 || stats.native_calls > 0,
            "flat mutable list loop should run through native whole-function or OSR path; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_selects_outer_loop_with_list_sort_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    let mut outer = 0
    let mut total = 0

    while outer < 20 {
        local values = List<Int>.new()
        let mut i = 0
        while i < 8 {
            let v = 8 - i
            List.push<Int>(list: mut values, value: read v)
            i = i + 1
        }
        List.sort(list: mut values)
        total = total + List.get<Int>(list: read values, index: 0)
        outer = outer + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("outer List.sort loop should be selected for OSR");
        assert!(
            func.code[candidate.header..candidate.exit]
                .iter()
                .any(|instr| matches!(instr, RegInstr::ListSort { .. })),
            "candidate should include List.sort, not just the inner builder loop; region={:#?}",
            &func.code[candidate.header..candidate.exit],
        );
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "List.sort<Int> loop should translate through the native helper framework; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert_eq!(out.stdout.trim(), "20");
        assert!(
            stats.osr_entries > 0,
            "List.sort<Int> loop should OSR-enter via native helper; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_transactional_map_get_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local table = Map<Int, Int>.new()
    let mut i = 0
    while i < 32 {
        let value = i * 3
        Map.insert<Int, Int>(map: mut table, key: read i, value: read value)
        i = i + 1
    }

    i = 0
    let mut total = 0
    while i < 32 {
        match Map.get<Int, Int>(map: read table, key: read i) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1000
            }
        }
        i = i + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("map read loop should be selected for OSR");
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected map loop should translate to OSR native IR; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "Map<Int,Int>.get match loop should OSR-enter via collection helpers; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "1488");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_transactional_set_contains_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local seen = Set.new<Int>()
    let limit = 2000
    let mut i = 0
    while i < limit {
        Set.insert(set: mut seen, value: read i)
        i = i + 1
    }

    i = 0
    let mut total = 0
    while i < 32 {
        if Set.contains(set: read seen, value: read i) {
            total = total + 1
        }
        i = i + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("set contains loop should be selected for OSR");
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected set loop should translate to OSR native IR; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "Set.contains<Int> loop should OSR-enter via collection helpers; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "32");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_transactional_sorted_set_contains_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local seen = SortedSet.new<Int>()
    let limit = 2000
    let mut i = 0
    while i < limit {
        let _inserted = SortedSet.insert<Int>(set: mut seen, value: read i)
        i = i + 1
    }

    i = 0
    let mut total = 0
    while i < 32 {
        if SortedSet.contains<Int>(set: read seen, value: read i) {
            total = total + 1
        }
        i = i + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("sorted-set contains loop should be selected for OSR");
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected sorted-set loop should translate to OSR native IR; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "SortedSet.contains<Int> loop should OSR-enter via collection helpers; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "32");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translates_sorted_set_len_int_through_list_len_helper() {
        let source = r#"

fn sum_len(seen: read SortedSet<Int>) -> Int {
    let mut i = 0
    let mut total = 0
    while i < SortedSet.len<Int>(set: read seen) {
        total = total + i
        i = i + 1
    }
    return total
}

fn main(args: read List<String>) -> Unit {
    local seen = SortedSet.new<Int>()
    let mut i = 0
    while i < 32 {
        let _inserted = SortedSet.insert<Int>(set: mut seen, value: read i)
        i = i + 1
    }

    let total = sum_len(seen: read seen)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["sum_len"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("SortedSet.len helper function should translate to native IR");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::ListLen,
                    ..
                }
            )),
            "loop-invariant SortedSet.len should lazily memoize the ListLen helper; code={:#?}",
            jit.code,
        );
        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "sum_len should run through whole-function native JIT; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "496");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translates_collection_is_empty_helpers() {
        let source = r#"

fn collection_empty_score(
    values: read List<Int>,
    table: read Map<Int, Int>,
    set: read Set<Int>,
    sorted: read SortedSet<Int>,
    sorted_table: read SortedMap<Int, Int>,
    queue: read Deque<Int>
) -> Int {
    let mut total = 0
    if List.is_empty<Int>(list: read values) {
        total = total + 1
    } else {
        total = total + 10
    }
    if Map.is_empty<Int, Int>(map: read table) {
        total = total + 2
    } else {
        total = total + 20
    }
    if Set.is_empty<Int>(set: read set) {
        total = total + 4
    } else {
        total = total + 40
    }
    if SortedSet.is_empty<Int>(set: read sorted) {
        total = total + 8
    } else {
        total = total + 80
    }
    if SortedMap.is_empty<Int, Int>(map: read sorted_table) {
        total = total + 16
    } else {
        total = total + 160
    }
    if Deque.is_empty<Int>(deque: read queue) {
        total = total + 32
    } else {
        total = total + 320
    }
    return total
}

fn main(args: read List<String>) -> Unit {
    local values = List<Int>.new()
    local table = Map<Int, Int>.new()
    local set = Set.new<Int>()
    local sorted = SortedSet.new<Int>()
    local sorted_table = SortedMap<Int, Int>.new()
    local queue = Deque<Int>.new()

    let empty_score = collection_empty_score(
        values: read values,
        table: read table,
        set: read set,
        sorted: read sorted,
        sorted_table: read sorted_table,
        queue: read queue
    )

    List.push<Int>(list: mut values, value: read 1)
    Map.insert<Int, Int>(map: mut table, key: read 1, value: read 2)
    Set.insert(set: mut set, value: read 3)
    let _sorted_inserted = SortedSet.insert<Int>(set: mut sorted, value: read 4)
    SortedMap.insert<Int, Int>(map: mut sorted_table, key: read 5, value: read 6)
    Deque.push_back<Int>(deque: mut queue, value: read 7)

    let non_empty_score = collection_empty_score(
        values: read values,
        table: read table,
        set: read set,
        sorted: read sorted,
        sorted_table: read sorted_table,
        queue: read queue
    )

    Output.write(message: read String.from_int(value: empty_score + non_empty_score))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["collection_empty_score"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("collection is_empty helper function should translate to native IR");
        for expected in [
            vm_jit::HostHelper::ListIsEmpty,
            vm_jit::HostHelper::MapIsEmpty,
            vm_jit::HostHelper::SetIsEmpty,
            vm_jit::HostHelper::SortedSetIsEmpty,
            vm_jit::HostHelper::SortedMapIsEmpty,
            vm_jit::HostHelper::DequeIsEmpty,
        ] {
            assert!(
                jit.code.iter().any(|instr| matches!(
                    instr,
                    vm_jit::JitInstr::HostCall { helper, .. } if *helper == expected
                )),
                "{expected:?} should lower through the generic host-helper path; code={:#?}",
                jit.code,
            );
        }

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "collection_empty_score should run through whole-function native JIT; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "693");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translates_map_and_set_len_helpers() {
        let source = r#"

fn map_set_len_score(table: read Map<Int, Int>, set: read Set<Int>) -> Int {
    return (Map.len<Int, Int>(map: read table) * 10) + Set.len<Int>(set: read set)
}

fn main(args: read List<String>) -> Unit {
    local table = Map<Int, Int>.new()
    local set = Set.new<Int>()
    let empty_score = map_set_len_score(table: read table, set: read set)

    Map.insert<Int, Int>(map: mut table, key: read 1, value: read 2)
    Set.insert(set: mut set, value: read 3)
    let non_empty_score = map_set_len_score(table: read table, set: read set)

    Output.write(message: read String.from_int(value: empty_score + non_empty_score))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["map_set_len_score"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("Map.len/Set.len helper function should translate to native IR");
        for expected in [vm_jit::HostHelper::MapLen, vm_jit::HostHelper::SetLen] {
            assert!(
                jit.code.iter().any(|instr| matches!(
                    instr,
                    vm_jit::JitInstr::HostCall { helper, .. } if *helper == expected
                )),
                "{expected:?} should lower through the generic host-helper path; code={:#?}",
                jit.code,
            );
        }

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "map_set_len_score should run through whole-function native JIT; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "11");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_folds_non_escaping_bytes_slice_len() {
        let source = r#"

fn bytes_slice_score(data: read Bytes, reps: Int) -> Int {
    let mut i = 0
    let mut total = 0
    while i < reps {
        let head = Bytes.slice(value: read data, start: 1, len: 4)
        total = total + Bytes.len(value: read head) + Bytes.len(value: read data)
        i = i + 1
    }
    return total
}

fn main(args: read List<String>) -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    let total = bytes_slice_score(data: read data, reps: 3)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["bytes_slice_score"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("Bytes.slice/len function should translate to native IR");
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::BytesSlice,
                    ..
                } | vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::BytesSlice,
                    ..
                }
            )),
            "non-escaping Bytes.slice must be dissolved before native lowering; code={:#?}",
            jit.code,
        );
        let memoized_lengths = jit
            .code
            .iter()
            .filter(|instr| {
                matches!(
                    instr,
                    vm_jit::JitInstr::MemoizedHostCall {
                        helper: vm_jit::HostHelper::BytesLen,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            memoized_lengths, 2,
            "both invariant source-length reads should execute at most once per invocation"
        );

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "bytes_slice_score should run through whole-function native JIT; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "30");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_folded_bytes_slice_len_matches_clamped_edge_semantics() {
        let source = r#"

fn edge_score(data: read Bytes) -> Int {
    let negative_start = Bytes.slice(value: read data, start: -5, len: 2)
    let saturating_len = Bytes.slice(value: read data, start: 1, len: 9223372036854775807)
    let past_end = Bytes.slice(value: read data, start: 9223372036854775807, len: 3)
    let negative_len = Bytes.slice(value: read data, start: 2, len: -9)
    return Bytes.len(value: read negative_start)
        + Bytes.len(value: read saturating_len)
        + Bytes.len(value: read past_end)
        + Bytes.len(value: read negative_len)
}

fn main(args: read List<String>) -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    Output.write(message: read String.from_int(value: edge_score(data: read data)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["edge_score"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("edge-case Bytes.slice/len function should translate");
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::BytesSlice,
                    ..
                } | vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::BytesSlice,
                    ..
                }
            )),
            "all non-escaping slices should be scalarized; code={:#?}",
            jit.code,
        );

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("edge-case program should run natively");
        assert!(stats.native_calls > 0, "edge_score must enter native code");
        assert_eq!(out.stdout.trim(), "7");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_keeps_escaping_bytes_slice_allocation() {
        let source = r#"

fn retained_slice(data: read Bytes) -> Bytes {
    return Bytes.slice(value: read data, start: 1, len: 4)
}

fn main(args: read List<String>) -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    let head = retained_slice(data: read data)
    Output.write(message: read String.from_int(value: Bytes.len(value: read head)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["retained_slice"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("escaping Bytes.slice should remain native-helper eligible");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::BytesSlice,
                    ..
                }
            )),
            "an escaping Bytes result must retain its allocation; code={:#?}",
            jit.code,
        );

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("escaping-slice program should run");
        assert!(
            stats.native_calls > 0,
            "retained_slice must enter native code"
        );
        assert_eq!(out.stdout.trim(), "4");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_keeps_unrelated_dynamic_bytes_len_translatable() {
        let source = r#"

fn byte_count(data: read Bytes) -> Int {
    return Bytes.len(value: read data)
}

fn main(args: read List<String>) -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    Output.write(message: read String.from_int(value: byte_count(data: read data)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["byte_count"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("a direct dynamic Bytes.len must remain native eligible");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::BytesLen,
                    ..
                }
            )),
            "the unrelated length query must remain a validating helper; code={:#?}",
            jit.code,
        );

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("direct Bytes.len program should run");
        assert!(stats.native_calls > 0, "byte_count must enter native code");
        assert_eq!(out.stdout.trim(), "6");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translates_string_slice_helper() {
        let source = r#"

fn string_slice_score(value: read String, reps: Int) -> Int {
    let mut i = 0
    let mut total = 0
    while i < reps {
        let head = String.slice(value: read value, start: 0, len: 4)
        total = total + String.len(value: read value) + String.len(value: read head)
        i = i + 1
    }
    return total
}

fn main(args: read List<String>) -> Unit {
    let total = string_slice_score(value: read "abcdef", reps: 3)
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["string_slice_score"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("String.slice helper function should translate to native IR");
        for expected in [
            vm_jit::HostHelper::StringSlice,
            vm_jit::HostHelper::StringLen,
        ] {
            assert!(
                jit.code.iter().any(|instr| matches!(
                    instr,
                    vm_jit::JitInstr::HostCall { helper, .. }
                        | vm_jit::JitInstr::MemoizedHostCall { helper, .. }
                        if *helper == expected
                )),
                "{expected:?} should lower through the generic host-helper path; code={:#?}",
                jit.code,
            );
        }

        let (out, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "string_slice_score should run through whole-function native JIT; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "30");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_transactional_sorted_map_insert_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local table = SortedMap<Int, Int>.new()
    let mut i = 0
    while i < 32 {
        let value = i * 2
        SortedMap.insert<Int, Int>(map: mut table, key: read i, value: read value)
        i = i + 1
    }

    Output.write(message: read String.from_int(value: SortedMap.len<Int, Int>(map: read table)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("sorted-map insert loop should be selected for OSR");
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected sorted-map insert loop should translate to OSR native IR; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "SortedMap.insert<Int,Int> loop should OSR-enter via collection helpers; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "32");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_sorted_map_get_match_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local table = SortedMap<Int, Int>.new()
    let mut i = 0
    while i < 32 {
        let value = i * 3
        SortedMap.insert<Int, Int>(map: mut table, key: read i, value: read value)
        i = i + 1
    }

    i = 0
    let mut total = 0
    while i < 32 {
        match SortedMap.get<Int, Int>(map: read table, key: read i) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1000
            }
        }
        i = i + 1
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("sorted-map get loop should be selected for OSR");
        let jit = translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected sorted-map get loop should translate to OSR native IR; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
        let telemetry = super::super::tier::NativeCompileTelemetry::from_jit_function(&jit.0);
        assert_eq!(telemetry.fused_map_match_helper_sites, 1);
        assert_eq!(
            telemetry.runtime_helper_call_sites, 1,
            "one sorted-map match must cross exactly one host-helper boundary",
        );
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "SortedMap.get<Int,Int> match loop should OSR-enter via fused helper; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "1488");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_map_get_match_int_distinguishes_zero_hit_from_miss() {
        let mut entries = ValueMap::default();
        entries.insert(jit_int_key(1), VmValue::Int(0));
        entries.insert(jit_int_key(2), VmValue::Int(22));
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(VmValue::Map(Rc::new(RefCell::new(entries))));

        let mut found = -1;
        assert_eq!(
            rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 1, &mut found),
            0
        );
        assert_eq!(found, 1);
        assert_eq!(
            rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 2, &mut found),
            22
        );
        assert_eq!(found, 1);
        assert_eq!(
            rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 3, &mut found),
            0
        );
        assert_eq!(found, 0);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_sorted_map_get_int_cache_handles_sequential_scan_and_fallback() {
        let map = sorted_map_value(vec![
            (VmValue::Int(1), VmValue::Int(10)),
            (VmValue::Int(3), VmValue::Int(30)),
            (VmValue::Int(7), VmValue::Int(70)),
        ]);
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(map);
        JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
            cache.borrow_mut().take();
        });

        let mut found = -1;
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 1, &mut found),
            10
        );
        assert_eq!(found, 1);
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 3, &mut found),
            30
        );
        assert_eq!(found, 1);
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 7, &mut found),
            70
        );
        assert_eq!(found, 1);

        // Non-sequential access must still fall back to the binary-search path.
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 3, &mut found),
            30
        );
        assert_eq!(found, 1);
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 4, &mut found),
            0
        );
        assert_eq!(found, 0);

        JIT_SORTED_MAP_SCAN_CACHE.with(|cache| {
            cache.borrow_mut().take();
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_closure_input_reads_resolve_current_heap_arg_attempt() {
        {
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::Closure(Rc::new(VmClosure {
                function: 4,
                captures: vec![VmValue::Int(10)],
            })));
            assert_eq!(rss_jit_closure_id(JitCallCtx::active_token(), 0), 4);
            assert_eq!(
                rss_jit_closure_capture(JitCallCtx::active_token(), 0, 0),
                10
            );
        }

        {
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::Closure(Rc::new(VmClosure {
                function: 9,
                captures: vec![VmValue::Int(30)],
            })));
            assert_eq!(
                rss_jit_closure_id(JitCallCtx::active_token(), 0),
                9,
                "handle 0 in a new native attempt must resolve the new closure, not a stale cached one",
            );
            assert_eq!(
                rss_jit_closure_capture(JitCallCtx::active_token(), 0, 0),
                30
            );
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_input_only_closure_read_rejects_output_handles() {
        let mut tx = JitHeapTransactionGuard::begin();
        let handle = rss_jit_string_from_int(JitCallCtx::active_token(), 42);
        assert!(handle < 0, "string helper should return an output handle");
        assert_eq!(
            rss_jit_closure_id(JitCallCtx::active_token(), handle),
            -1,
            "closure-id helper is input-handle-only and must not resolve output handles",
        );
        tx.abort();
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_field_closure_reads_do_not_materialize_field_handle() {
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(VmValue::Struct(Rc::new(VmStruct::from_named(
            Rc::from("Op"),
            vec![(
                "apply".to_string(),
                VmValue::Closure(Rc::new(VmClosure {
                    function: 12,
                    captures: vec![VmValue::Int(77)],
                })),
            )],
        ))));

        assert_eq!(
            rss_jit_field_closure_id(JitCallCtx::active_token(), 0, 0),
            12
        );
        assert_eq!(
            rss_jit_field_closure_capture(JitCallCtx::active_token(), 0, 0, 0),
            77
        );
        assert_eq!(
            rss_jit_field_closure_id(JitCallCtx::active_token(), 0, 99),
            -1,
            "field closure id is total like closure_id",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_map_handle_cache_clears_between_heap_arg_attempts() {
        let first = Rc::new(RefCell::new(ValueMap::default()));
        first.borrow_mut().insert(jit_int_key(1), VmValue::Int(10));
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::Map(Rc::clone(&first)));
            let mut found = -1;
            assert_eq!(
                rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 1, &mut found),
                10
            );
            assert_eq!(found, 1);
            assert_eq!(
                rss_jit_map_insert_int(JitCallCtx::active_token(), 0, 2, 20),
                0
            );
            assert_eq!(first.borrow().get(&jit_int_key(2)), Some(&VmValue::Int(20)));
            tx.commit_scalar_with_writebacks(&[])
                .expect("map helper transaction should commit");
        }

        let second = Rc::new(RefCell::new(ValueMap::default()));
        second.borrow_mut().insert(jit_int_key(1), VmValue::Int(30));
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::Map(Rc::clone(&second)));
            let mut found = -1;
            assert_eq!(
                rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 1, &mut found),
                30,
                "handle 0 in a new native attempt must resolve the new heap arg, not the cached prior map",
            );
            assert_eq!(found, 1);
            assert_eq!(
                rss_jit_map_insert_int(JitCallCtx::active_token(), 0, 2, 40),
                0
            );
            assert_eq!(
                second.borrow().get(&jit_int_key(2)),
                Some(&VmValue::Int(40))
            );
            assert_eq!(
                first.borrow().get(&jit_int_key(2)),
                Some(&VmValue::Int(20)),
                "new-attempt writes must not hit the stale cached map",
            );
            tx.commit_scalar_with_writebacks(&[])
                .expect("map helper transaction should commit");
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_deque_handle_cache_clears_between_heap_arg_attempts() {
        let first = Rc::new(RefCell::new(VecDeque::from(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ])));
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::Deque(Rc::clone(&first)));
            assert_eq!(rss_jit_deque_len(JitCallCtx::active_token(), 0), 2);
            assert_eq!(
                rss_jit_deque_push_back_int(JitCallCtx::active_token(), 0, 3),
                0
            );
            assert_eq!(first.borrow().len(), 3);
            tx.commit_scalar_with_writebacks(&[])
                .expect("deque helper transaction should commit");
        }

        let second = Rc::new(RefCell::new(VecDeque::from(vec![VmValue::Int(10)])));
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::Deque(Rc::clone(&second)));
            assert_eq!(
                rss_jit_deque_len(JitCallCtx::active_token(), 0),
                1,
                "handle 0 in a new native attempt must resolve the new heap arg, not the cached prior deque",
            );
            assert_eq!(
                rss_jit_deque_push_back_int(JitCallCtx::active_token(), 0, 11),
                0
            );
            assert_eq!(second.borrow().len(), 2);
            assert_eq!(
                first.borrow().len(),
                3,
                "new-attempt writes must not hit the stale cached deque",
            );
            tx.commit_scalar_with_writebacks(&[])
                .expect("deque helper transaction should commit");
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_list_handle_cache_clears_between_heap_arg_attempts() {
        let first = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ])));
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::List(Rc::clone(&first)));
            assert_eq!(rss_jit_list_len(JitCallCtx::active_token(), 0), 2);
            assert_eq!(rss_jit_list_push_int(JitCallCtx::active_token(), 0, 3), 0);
            assert_eq!(first.borrow().len(), 3);
            tx.commit_scalar_with_writebacks(&[])
                .expect("list helper transaction should commit");
        }

        let second = Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(10)])));
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::List(Rc::clone(&second)));
            assert_eq!(
                rss_jit_list_len(JitCallCtx::active_token(), 0),
                1,
                "handle 0 in a new native attempt must resolve the new heap arg, not the cached prior list",
            );
            assert_eq!(rss_jit_list_push_int(JitCallCtx::active_token(), 0, 11), 0);
            assert_eq!(second.borrow().len(), 2);
            assert_eq!(
                first.borrow().len(),
                3,
                "new-attempt writes must not hit the stale cached list",
            );
            tx.commit_scalar_with_writebacks(&[])
                .expect("list helper transaction should commit");
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_list_handle_cache_clears_output_handles_when_transaction_resets() {
        {
            let mut tx = JitHeapTransactionGuard::begin();
            let first = rss_jit_list_new_int(JitCallCtx::active_token());
            assert!(first < 0, "new list helper should return an output handle");
            assert_eq!(
                rss_jit_list_push_int(JitCallCtx::active_token(), first, 1),
                0
            );
            assert_eq!(rss_jit_list_len(JitCallCtx::active_token(), first), 1);
            tx.abort();
        }

        {
            let mut tx = JitHeapTransactionGuard::begin();
            let second = rss_jit_list_new_int(JitCallCtx::active_token());
            assert!(second < 0, "new list helper should return an output handle");
            assert_eq!(
                rss_jit_list_len(JitCallCtx::active_token(), second),
                0,
                "reused output handle must resolve the new staged list, not a stale cached list",
            );
            tx.abort();
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_transactional_deque_pop_front_int() {
        let source = r#"

fn main(args: read List<String>) -> Unit {
    local q = Deque<Int>.new()
    let mut i = 0
    while i < 32 {
        Deque.push_back<Int>(deque: mut q, value: read i)
        i = i + 1
    }

    let mut total = 0
    while Deque.len<Int>(deque: read q) > 0 {
        match Deque.pop_front<Int>(deque: mut q) {
            Some(value) => {
                total = total + value
            }
            None => {
                total = total - 1000
            }
        }
    }

    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("deque pop loop should be selected for OSR");
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            std::iter::empty::<(String, ExternalFunction)>().collect(),
        );
        vm.set_limits(VmLimits::unbounded_for_trusted_host());
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        assert_eq!(
            vm.resolve_osr_candidate(func),
            Some(candidate.header),
            "resolver should arm the selected deque loop",
        );
        let (code, n_regs, ip_map, _) = native_scalar_replace_options_in_region(
            &func.code,
            func.regs,
            candidate.header,
            candidate.exit,
        )
        .expect("deque option loop should scalar-replace");
        let transformed_header = ip_map
            .iter()
            .position(|&old_ip| old_ip == candidate.header)
            .expect("transformed code should preserve the loop header");
        let transformed_loop = detect_natural_loop_at(&code, transformed_header)
            .expect("transformed deque loop should remain reducible");
        translate_osr_loop(&code, n_regs, func.params, func.captures, transformed_loop)
            .unwrap_or_else(|| {
                panic!(
                    "transformed deque loop should translate to OSR native IR; region={:#?}",
                    &code[transformed_loop.header..transformed_loop.exit],
                )
            });
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "Deque.pop_front<Int> loop should OSR-enter via collection helpers; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "496");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_loop_with_field_set_int_handle_update() {
        let source = r#"
struct Box {
    value: Int
}

fn main(args: read List<String>) -> Unit {
    let mut box = Box(value: 0)
    let mut i = 0
    let mut total = 0
    while i < 32 {
        box.value = i
        total = total + box.value
        i = i + 1
    }
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let (_out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "SetFieldSlot<Int> loop should OSR-enter via copy-on-write helper; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_restores_field_set_int_handle_live_after_loop() {
        let source = r#"
struct Box {
    value: Int
}

fn main(args: read List<String>) -> Unit {
    let mut box = Box(value: 0)
    let mut i = 0
    while i < 32 {
        box.value = i
        i = i + 1
    }
    Output.write(message: read String.from_int(value: box.value))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "copy-updated struct handle live-out should OSR-enter and restore through heap writeback; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "31");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_field_set_int_on_mut_parameter_writes_back_to_caller() {
        let source = r#"
struct Box {
    value: Int
}

fn bump(box: mut Box) -> Int {
    box.value = box.value + 1
    return box.value
}

fn main(args: read List<String>) -> Unit {
    let mut box = Box(value: 0)
    let _value = bump(box: mut box)
    Output.write(message: read String.from_int(value: box.value))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let bump = executable.unit.function_ids["bump"];
        let bump_func = executable.unit.functions[bump].as_ref();
        assert!(
            translate_to_native_jit(&executable.unit, bump_func).is_some(),
            "SetFieldSlot on a parameter should be native once heap writeback materialization exists; code={:#?}",
            bump_func.code,
        );
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.native_calls > 0,
            "bump should run whole-function native; stats={stats:?}"
        );
        assert_eq!(out.stdout.trim(), "1");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_field_set_int_on_mut_parameter_writes_back_to_caller() {
        let source = r#"
struct Box {
    value: Int
}

fn bump_loop(box: mut Box, limit: Int) -> Unit {
    let mut i = 0
    while i < limit {
        box.value = i
        i = i + 1
    }
    return Unit
}

fn main(args: read List<String>) -> Unit {
    let mut box = Box(value: 0)
    bump_loop(box: mut box, limit: 32)
    Output.write(message: read String.from_int(value: box.value))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let bump_loop = executable.unit.function_ids["bump_loop"];
        let func = executable.unit.functions[bump_loop].as_ref();
        let candidate = detect_natural_loops(&func.code)
            .into_iter()
            .find(|lp| {
                func.code
                    .get(lp.header..lp.exit)
                    .is_some_and(|region| region.iter().all(native_subset_instruction))
            })
            .expect("test should expose a raw native-subset loop");
        assert!(
            translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
                .is_some(),
            "SetFieldSlot on a parameter should be OSR-native once heap writeback materialization exists",
        );
        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");
        assert!(
            stats.osr_entries > 0,
            "bump_loop should OSR-enter; stats={stats:?}"
        );
        assert_eq!(out.stdout.trim(), "31");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translates_bool_returning_mut_struct_list_write_helper() {
        let source = r#"

struct MailboxInt {
    capacity: Int
    head: Int
    count: Int
    values: List<Int>
}

fn mailbox_send(m: mut MailboxInt, value: Int) -> Bool {
    if m.count >= m.capacity {
        return false
    }
    let tail = (m.head + m.count) % m.capacity
    if tail < List.len<Int>(list: read m.values) {
        List.set<Int>(list: mut m.values, index: tail, value: read value)
    } else {
        List.push<Int>(list: mut m.values, value: read value)
    }
    m.count = m.count + 1
    return true
}

fn mailbox_take(m: mut MailboxInt) -> Option<Int> {
    if m.count <= 0 {
        return None
    }
    let value = List.get<Int>(list: read m.values, index: m.head)
    m.head = (m.head + 1) % m.capacity
    m.count = m.count - 1
    return Some(value)
}

fn hot(x: Int) -> Int {
    return x
}

fn main(args: read List<String>) -> Unit {
    let value = hot(x: 1)
    Output.write(message: read String.from_int(value: value))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let send = executable.unit.function_ids["mailbox_send"];
        let send_func = executable.unit.functions[send].as_ref();
        let take = executable.unit.function_ids.get("mailbox_take").copied();
        if let Some(take) = take {
            let take_func = executable.unit.functions[take].as_ref();
            assert!(
                native_callee_inlinable_j3(take_func, take_func.params),
                "mailbox_take should be J3-inlinable; code={:#?}",
                take_func.code,
            );
        }
        assert!(
            translate_to_native_jit(&executable.unit, send_func).is_some(),
            "mailbox_send should translate with Bool return, list writes, and mut-struct writeback; code={:#?}",
            send_func.code,
        );
    }
