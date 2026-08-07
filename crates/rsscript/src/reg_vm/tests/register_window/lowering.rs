    #[cfg(feature = "native-jit")]
    #[test]
    fn pure_captureless_closure_fast_path_covers_list_pipeline_shapes() {
        let acc_layout = native_test_layout("Acc", &["total"]);
        let unit = native_test_unit(vec![
            native_test_function(
                "map_value",
                1,
                4,
                vec![
                    RegInstr::LoadInt { dst: 1, value: 2 },
                    RegInstr::MulInt {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    RegInstr::LoadInt { dst: 1, value: 1 },
                    RegInstr::AddInt {
                        dst: 3,
                        lhs: 2,
                        rhs: 1,
                    },
                    RegInstr::Return { src: 3 },
                ],
            ),
            native_test_function(
                "filter_value",
                1,
                5,
                vec![
                    RegInstr::LoadInt { dst: 1, value: 2 },
                    RegInstr::DivInt {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    RegInstr::MulInt {
                        dst: 3,
                        lhs: 2,
                        rhs: 1,
                    },
                    RegInstr::NotEqual {
                        dst: 4,
                        lhs: 3,
                        rhs: 0,
                    },
                    RegInstr::Return { src: 4 },
                ],
            ),
            native_test_function(
                "fold_acc",
                2,
                4,
                vec![
                    RegInstr::GetFieldSlot {
                        dst: 2,
                        base: 0,
                        slot: 0,
                    },
                    RegInstr::AddInt {
                        dst: 2,
                        lhs: 2,
                        rhs: 1,
                    },
                    RegInstr::MakeStruct {
                        dst: 3,
                        layout: Rc::clone(&acc_layout),
                        fields: vec![("total".to_string(), 2)],
                    },
                    RegInstr::Return { src: 3 },
                ],
            ),
        ]);
        let vm = RegVm::new(
            Rc::new(native_test_unit(Vec::new())),
            Vec::<String>::new(),
            std::iter::empty::<(String, ExternalFunction)>().collect(),
        );

        let map = VmClosure {
            function: 0,
            captures: Vec::new(),
        };
        let mapped = vm
            .try_call_captureless_pure_closure(&unit, &map, &[VmValue::Int(20)])
            .expect("map closure should use pure fast path")
            .expect("map closure should run");
        assert_eq!(mapped, VmValue::Int(41));

        let filter = VmClosure {
            function: 1,
            captures: Vec::new(),
        };
        let keep = vm
            .try_call_captureless_pure_closure(&unit, &filter, &[VmValue::Int(41)])
            .expect("filter closure should use pure fast path")
            .expect("filter closure should run");
        assert_eq!(keep, VmValue::Bool(true));

        let fold = VmClosure {
            function: 2,
            captures: Vec::new(),
        };
        let state = VmValue::Struct(Rc::new(VmStruct::with_layout(
            Rc::clone(&acc_layout),
            vec![VmValue::Int(10)],
        )));
        let folded = vm
            .try_call_captureless_pure_closure(&unit, &fold, &[state, VmValue::Int(7)])
            .expect("fold closure should use pure fast path")
            .expect("fold closure should run");
        let VmValue::Struct(data) = folded else {
            panic!("fold closure should return an Acc struct");
        };
        assert_eq!(data.get("total"), Some(&VmValue::Int(17)));

        let state = VmValue::Struct(Rc::new(VmStruct::with_layout(
            Rc::clone(&acc_layout),
            vec![VmValue::Int(10)],
        )));
        let list = Rc::new(RefCell::new(TypedVec::Ints(vec![1, 2, 3, 4])));
        let folded =
            RegVm::try_fold_int_list_with_struct_plan_for_test(&unit, &list, &state, &fold)
                .expect("Int-list + Int-struct fold should use scalar struct fold path")
                .expect("scalar struct fold should run");
        let VmValue::Struct(data) = folded else {
            panic!("scalar struct fold should return an Acc struct");
        };
        assert_eq!(data.get("total"), Some(&VmValue::Int(20)));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_list_pipeline_bench_sources_match_vm_output() {
        for (file, arg, expected_stdout) in [
            ("list_closure_pipeline.rss", "64", "4096\n"),
            ("pipeline_chain.rss", "64", "3008\n"),
        ] {
            let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../benchmarks/micro")
                .join(file);
            let source = std::fs::read_to_string(&source_path)
                .unwrap_or_else(|error| panic!("failed to read {source_path:?}: {error}"));
            let executable = reg_vm_compile_source(file, &source).expect("lowering should succeed");

            let vm_out = executable
                .eval_main_with_args([arg])
                .expect("VM benchmark source should run");
            let (native_out, _stats) = executable
                .eval_main_with_args_native_osr_with_stats([arg])
                .expect("native benchmark source should run");

            assert_eq!(vm_out.stdout, expected_stdout, "{file} VM stdout changed");
            assert_eq!(
                native_out.stdout, vm_out.stdout,
                "{file} native stdout should match VM"
            );
        }
    }

    #[test]
    fn lowering_omits_deep_copy_for_scalar_params_only() {
        let source = r#"

fn scalar(n: Int, ok: Bool) -> Int {
    if ok {
        return n + 1
    }
    return n
}

fn list_len(xs: read List<Int>) -> Int {
    return List.len<Int>(list: read xs)
}

fn main() -> Unit {
    local xs = List.new<Int>()
    List.push<Int>(list: mut xs, value: read 1)
    let a = scalar(n: 41, ok: true)
    let b = list_len(xs: read xs)
    Output.write(message: read String.from_int(value: a + b))
    return Unit
}
"#;
        let executable = reg_vm_compile_source("scalar-param-copy.rss", source)
            .expect("lowering should succeed");
        let scalar_id = executable.unit.function_ids["scalar"];
        let scalar = executable.unit.functions[scalar_id].as_ref();
        assert!(
            !scalar
                .code
                .iter()
                .any(|instr| matches!(instr, RegInstr::DeepCopy { .. })),
            "primitive scalar params should not emit prologue DeepCopy: {:#?}",
            scalar.code,
        );

        let list_len_id = executable.unit.function_ids["list_len"];
        let list_len = executable.unit.functions[list_len_id].as_ref();
        // A `read` heap/value param always carries a prologue copy-isolation MARKER in
        // its slot: an eager `DeepCopy` when elision is off, or a neutralized
        // `DeepCopyElided` when the compile-time elision pass (default ON) proves the
        // copy redundant. Either way the slot is present (scalars emit neither) — this
        // is the scalar-vs-heap distinction the test guards, independent of the elision
        // flag. The elision-specific assertion lives in
        // `deepcopy_elision_fires_for_read_only_heap_param`.
        assert!(
            list_len.code.iter().any(|instr| matches!(
                instr,
                RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. }
            )),
            "read heap/value params must carry a copy-isolation marker: {:#?}",
            list_len.code,
        );
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "43\n");
    }

    /// Regression guard for the compile-time `DeepCopy`-elision perf win (the ~16x
    /// speedup on `benchmarks/vm-jit/kernels/deepcopy_read_param.rss`). Mirrors that
    /// kernel's hot shape: a NON-`mut` heap param (`g: read Bag`) used only through
    /// read paths (`GetField` on `g`, then `List.len`/`List.get`), called from another
    /// function. Under the DEFAULT (`RSS_VM_ELIDE_DEEPCOPY` unset ⇒ elision ON) the
    /// lowerer must PROVE the prologue `DeepCopy` of `g` redundant and neutralize it to
    /// `RegInstr::DeepCopyElided` — the marker that turns the per-call deep copy into a
    /// cheap `Rc` share. If a future change re-introduces the eager copy (elision stops
    /// firing) `DeepCopyElided` disappears and a raw `DeepCopy` returns, failing this
    /// test. Run with `RSS_VM_ELIDE_DEEPCOPY=0` to see the guarded regression: the
    /// elided marker is gone and the eager `DeepCopy` is back.
    #[test]
    fn deepcopy_elision_fires_for_read_only_heap_param() {
        let source = r#"

struct Bag {
    a: List<Int>,
    b: List<Int>,
}

fn sum_reads(g: read Bag, i: Int) -> Int {
    let na = List.len<Int>(list: read g.a)
    let nb = List.len<Int>(list: read g.b)
    let va = List.get<Int>(list: read g.a, index: i % na)
    let vb = List.get<Int>(list: read g.b, index: i % nb)
    return va + vb
}

fn caller(g: read Bag) -> Int {
    return sum_reads(g: read g, i: 0)
}

fn main() -> Unit {
    local a = List.new<Int>()
    List.push<Int>(list: mut a, value: read 7)
    local b = List.new<Int>()
    List.push<Int>(list: mut b, value: read 5)
    let bag = Bag(a: take a, b: take b)
    Output.write(message: read String.from_int(value: caller(g: read bag)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("deepcopy-elision.rss", source).expect("lowering should succeed");

        let sum_reads_id = executable.unit.function_ids["sum_reads"];
        let sum_reads = executable.unit.functions[sum_reads_id].as_ref();
        let elided = sum_reads
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopyElided { .. }))
            .count();
        let eager = sum_reads
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopy { .. }))
            .count();
        // Under the default the read-only heap param's copy is elided; the elision is
        // gated (`elide_deepcopy_enabled`) so an explicit `RSS_VM_ELIDE_DEEPCOPY=0`
        // process would instead leave the eager `DeepCopy` — which is exactly the
        // regression this guard is meant to catch.
        if crate::reg_vm::model::elide_deepcopy_enabled_for_test() {
            assert!(
                elided >= 1,
                "elision ON: read-only heap param DeepCopy should be neutralized to \
                 DeepCopyElided, got {elided} elided / {eager} eager: {:#?}",
                sum_reads.code,
            );
            assert_eq!(
                eager, 0,
                "elision ON: no eager DeepCopy should remain for the read-only heap \
                 param: {:#?}",
                sum_reads.code,
            );
        } else {
            assert!(
                eager >= 1 && elided == 0,
                "elision OFF: eager DeepCopy must be retained (perf-win regression path): \
                 got {elided} elided / {eager} eager: {:#?}",
                sum_reads.code,
            );
        }

        // Elision (or its absence) must never change observable behavior.
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "12\n");
    }

    /// SH-022 regression: a `read List<Char>` param whose only keep-forcing use is
    /// a pure scalar `Char.*` intrinsic (here `Char.to_code` on a `ListGet`-extracted
    /// element) must have its prologue `DeepCopy` ELIDED. Before the `Char.*`
    /// intrinsics were classified `PureFreshReader`, `Char.to_code(c)` pinned the
    /// copy, so every per-char lexer helper call deep-copied the whole char list —
    /// a genuine O(n^2) that made the self-hosted lexer ~5000x slower than native.
    #[test]
    fn deepcopy_elision_fires_for_char_list_read_param() {
        let source = r#"
fn scan(chars: read List<Char>, i: Int) -> Int {
    let c = List.get<Char>(list: read chars, index: i)
    return Char.to_code(value: read c)
}

fn main() -> Unit {
    let chars = String.chars(value: read "abc")
    Output.write(message: read String.from_int(value: scan(chars: read chars, i: 1)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("deepcopy-elision-char.rss", source).expect("lowering succeeds");

        let scan_id = executable.unit.function_ids["scan"];
        let scan = executable.unit.functions[scan_id].as_ref();
        let elided = scan
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopyElided { .. }))
            .count();
        let eager = scan
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopy { .. }))
            .count();
        if crate::reg_vm::model::elide_deepcopy_enabled_for_test() {
            assert!(
                elided >= 1 && eager == 0,
                "elision ON: `read List<Char>` param whose only use is ListGet + \
                 Char.to_code must be elided, got {elided} elided / {eager} eager: {:#?}",
                scan.code,
            );
        }

        // Elision must not change behavior: scan("abc"[1]) == code of 'b' == 98.
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "98\n");
    }

    /// Slice 1 generalization (beyond SH-022's Char special-case): a `read
    /// List<Int>` param whose extracted elements are RETURNED (a keep-forcing use)
    /// must still have its prologue `DeepCopy` ELIDED. Extracting a `Copy` scalar
    /// (`Int`) can no longer taint its source collection, so the returned element
    /// is untainted and nothing pins the copy — even though `Return` of a tainted
    /// register WOULD force a keep. This is the general kill for the O(n^2)
    /// `read List<Scalar>` copy class, independent of any per-scalar intrinsic
    /// classification. No `Char.*` (or any) intrinsic on the element is involved.
    #[test]
    fn deepcopy_elision_fires_for_int_list_read_param() {
        let source = r#"

fn scan(xs: read List<Int>, i: Int) -> Int {
    let a = List.get<Int>(list: read xs, index: i)
    let b = List.get<Int>(list: read xs, index: i + 1)
    if a > b {
        return a
    }
    return b
}

fn main() -> Unit {
    local xs = List.new<Int>()
    List.push<Int>(list: mut xs, value: read 3)
    List.push<Int>(list: mut xs, value: read 7)
    Output.write(message: read String.from_int(value: scan(xs: read xs, i: 0)))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("deepcopy-elision-int.rss", source).expect("lowering succeeds");

        let scan_id = executable.unit.function_ids["scan"];
        let scan = executable.unit.functions[scan_id].as_ref();
        let elided = scan
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopyElided { .. }))
            .count();
        let eager = scan
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopy { .. }))
            .count();
        if crate::reg_vm::model::elide_deepcopy_enabled_for_test() {
            assert!(
                elided >= 1 && eager == 0,
                "elision ON: `read List<Int>` param whose extracted elements are \
                 returned must be elided (scalar extraction must not taint the list), \
                 got {elided} elided / {eager} eager: {:#?}",
                scan.code,
            );
        }

        // Elision must not change behavior: max(xs[0], xs[1]) == max(3, 7) == 7.
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "7\n");
    }

    /// Slice 2: a scalar payload extracted by a PATTERN bind must not taint the
    /// scrutinee. Here `pick(opt: read Option<Int>)` unwraps `Some(v)` via
    /// `UnwrapSome`; because `Int` is a `Copy` scalar the unwrap is a bit-copy that
    /// cannot alias/escape the `Option`'s `Rc`, so `v` is marked scalar (`note_scalar`)
    /// and the prologue `DeepCopy` of the `read Option<Int>` param is ELIDED. Before
    /// Slice 2 the pattern lowerer didn't thread the scrutinee type, so `v` stayed
    /// tainted and `UnwrapSome` (unclassified) pinned the copy.
    #[test]
    fn deepcopy_elision_fires_for_option_scalar_pattern_bind() {
        let source = r#"
fn pick(opt: read Option<Int>) -> Int {
    match read opt {
        Some(v) => { return read v }
        None => { return 0 }
    }
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: pick(opt: read Some(42))))
    return Unit
}
"#;
        let executable = reg_vm_compile_source("deepcopy-elision-option.rss", source)
            .expect("lowering succeeds");

        let pick_id = executable.unit.function_ids["pick"];
        let pick = executable.unit.functions[pick_id].as_ref();
        let elided = pick
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopyElided { .. }))
            .count();
        let eager = pick
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopy { .. }))
            .count();
        if crate::reg_vm::model::elide_deepcopy_enabled_for_test() {
            assert!(
                elided >= 1 && eager == 0,
                "elision ON: `read Option<Int>` param whose `Some(v)` scalar payload is \
                 pattern-bound and returned must be elided (scalar unwrap must not taint \
                 the scrutinee), got {elided} elided / {eager} eager: {:#?}",
                pick.code,
            );
        }

        // Elision must not change behavior: pick(Some(42)) == 42.
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "42\n");
    }

    /// Slice 3 (borrow-by-default): a `read String` param whose only use is a
    /// proven-pure reader (`String.len`, now classified `PureFreshReader`) must have
    /// its prologue `DeepCopy` ELIDED. Before Slice 3 every `String.*` intrinsic hit
    /// the conservative `Keep` arm, so `String.len(read s)` pinned the copy and each
    /// call deep-copied the whole string. `String.len` returns a fresh `Int` and never
    /// mutates/stores/aliases its arg, so sharing the caller's `Rc` is sound.
    #[test]
    fn deepcopy_elision_fires_for_string_read_param() {
        let source = r#"
fn measure(s: read String) -> Int {
    return String.len(value: read s)
}

fn main() -> Unit {
    Output.write(message: read String.from_int(value: measure(s: read "hello")))
    return Unit
}
"#;
        let executable = reg_vm_compile_source("deepcopy-elision-string.rss", source)
            .expect("lowering succeeds");

        let measure_id = executable.unit.function_ids["measure"];
        let measure = executable.unit.functions[measure_id].as_ref();
        let elided = measure
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopyElided { .. }))
            .count();
        let eager = measure
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopy { .. }))
            .count();
        if crate::reg_vm::model::elide_deepcopy_enabled_for_test() {
            assert!(
                elided >= 1 && eager == 0,
                "elision ON: `read String` param whose only use is the pure reader \
                 `String.len` must be elided, got {elided} elided / {eager} eager: {:#?}",
                measure.code,
            );
        } else {
            assert!(
                eager >= 1 && elided == 0,
                "elision OFF: eager DeepCopy must be retained: got {elided} elided / \
                 {eager} eager: {:#?}",
                measure.code,
            );
        }

        // Elision must not change behavior: len("hello") == 5.
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "5\n");
    }

    /// Slice 3 NEGATIVE guard (over-promotion): a `read List<Int>` param that IS STORED
    /// into a struct (then reloaded and mutated in a loop) must KEEP its prologue
    /// `DeepCopy`, even though a promoted pure reader (`String.len` on a fresh string
    /// derived from it) is also called. Storing lowers to `MakeStruct`, an UNCLASSIFIED
    /// instruction that references the tainted param register and so hits the fail-safe
    /// `Keep` default — the param stays tainted and the copy is retained. This proves
    /// Slice 3 widened only the READ-ONLY-SAFE set, not the ESCAPE set: a storing op is
    /// still caught, and behavior is byte-for-byte unchanged (caller's `xs[0]` stays 7,
    /// the callee mutated only its own deep copy). Mirrors the shape of the native
    /// `native_store_reload_mutate_non_mut_heap_param_does_not_leak` leak guard.
    #[test]
    fn deepcopy_elision_kept_for_stored_read_param() {
        let source = r#"

struct Box {
    items: List<Int>
}

fn stash(xs: read List<Int>, n: Int) -> Int {
    let probe = String.len(value: read String.from_int(value: List.get<Int>(list: read xs, index: 0)))
    let b = Box(items: read xs)
    let mut i = 0
    while i < n {
        let mut inner = b.items
        List.set<Int>(list: mut inner, index: 0, value: read i)
        i = i + 1
    }
    return List.get<Int>(list: read xs, index: 0) + probe
}

fn main() -> Unit {
    local xs = List.new<Int>()
    List.push<Int>(list: mut xs, value: read 7)
    let r = stash(xs: read xs, n: read 3)
    Output.write(message: read String.from_int(value: List.get<Int>(list: read xs, index: 0)))
    Output.write(message: read String.from_int(value: r))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("deepcopy-elision-store.rss", source).expect("lowering succeeds");

        let stash_id = executable.unit.function_ids["stash"];
        let stash = executable.unit.functions[stash_id].as_ref();
        let elided = stash
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopyElided { .. }))
            .count();
        let eager = stash
            .code
            .iter()
            .filter(|instr| matches!(instr, RegInstr::DeepCopy { .. }))
            .count();
        // The store forces the copy to be kept regardless of the elision gate: with the
        // gate ON the DeepCopy must NOT be neutralized (elided == 0); with it OFF the eager
        // DeepCopy is likewise retained. Either way, no elision for the stored param.
        assert_eq!(
            elided, 0,
            "over-promotion guard: a `read Map` param stored into a struct must KEEP its \
             copy (no DeepCopyElided), got {elided} elided / {eager} eager: {:#?}",
            stash.code,
        );
        assert!(
            eager >= 1,
            "over-promotion guard: the eager DeepCopy of the stored `read Map` param must \
             be retained, got {elided} elided / {eager} eager: {:#?}",
            stash.code,
        );

        // Soundness check — elision must not leak to the caller: the caller's xs[0] stays 7
        // (line 1), proving the prologue copy was KEPT. The callee's own xs is a separate deep
        // copy; via the stored `b.items` alias the loop drives it to 2 (last i in 0..3), so
        // `xs[0] + probe == 2 + len("7") == 3` (line 2). Deterministic either way.
        let output = executable
            .eval_main_with_args(Vec::<String>::new())
            .expect("program should still run");
        assert_eq!(output.stdout, "7\n3\n");
    }

    #[test]
    fn jit_runs_scalar_self_recursion_on_flat_executor() {
        let source = r#"

fn fib(n: Int) -> Int {
    if n < 2 {
        return n
    }
    return fib(n: n - 1) + fib(n: n - 2)
}

fn main() -> Unit {
    let value = fib(n: 10)
    Output.write(message: read String.from_int(value: value))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("scalar-recursion.rss", source).expect("lowering should succeed");
        let output = executable
            .eval_main_with_args_jit_and_limits(
                Vec::<String>::new(),
                VmLimits::unbounded_for_trusted_host(),
            )
            .expect("JIT run should succeed");
        assert_eq!(output.stdout, "55\n");
        let fib_id = executable.unit.function_ids["fib"];
        assert_eq!(
            executable.unit.functions[fib_id]
                .jit_self_recursion_kind
                .get(),
            Some(crate::reg_vm::model::SelfRecursionKind::Int),
            "fib should be recognized as an Int scalar self-recursive JIT candidate",
        );
    }
