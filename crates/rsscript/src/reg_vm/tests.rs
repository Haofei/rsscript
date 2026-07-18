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

    /// Site 1: native-lowerable intrinsics are explicit opt-ins.
    #[test]
    fn native_lowerable_intrinsics_are_explicit() {
        for i in [
            RegIntrinsic::IntToFloat,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::StringLen,
            RegIntrinsic::StringSlice,
            RegIntrinsic::StringPadLeft,
            RegIntrinsic::StringSplit,
            RegIntrinsic::StringStartsWith,
            RegIntrinsic::BytesLen,
            RegIntrinsic::BytesSlice,
        ] {
            assert!(intrinsic_descriptor(i).native_lowerable);
        }
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::IntToFloat).effect,
            IntrinsicEffect::Pure
        );
        // A non-listed intrinsic is NOT native-lowerable.
        assert!(!intrinsic_descriptor(RegIntrinsic::ListContains).native_lowerable);
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
        assert!(
            intrinsic_descriptor(RegIntrinsic::StringLen)
                .combinator_kind
                .is_none()
        );
        assert!(
            intrinsic_descriptor(RegIntrinsic::ListContains)
                .combinator_kind
                .is_none()
        );
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
        assert!(
            intrinsic_descriptor(RegIntrinsic::StringCopy)
                .string_fold_role
                .is_none()
        );
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
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesLen).effect,
            IntrinsicEffect::Read
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesFromString).effect,
            IntrinsicEffect::Allocate
        );
        assert_eq!(
            intrinsic_descriptor(RegIntrinsic::BytesSlice).effect,
            IntrinsicEffect::Allocate
        );
        // A non-fold Bytes intrinsic has no Bytes role.
        assert!(
            intrinsic_descriptor(RegIntrinsic::BytesConcat)
                .bytes_fold_role
                .is_none()
        );
        // String-fold intrinsics carry no Bytes role.
        assert!(
            intrinsic_descriptor(RegIntrinsic::StringLen)
                .bytes_fold_role
                .is_none()
        );
    }

    /// The deopt cold-arm pure-builder whitelist: the four String builders plus the
    /// pure slice/pad/bytes Allocate producers. Each reads only its operands and is
    /// faithfully re-runnable on the interpreter after a native cold-arm `Bail`.
    #[test]
    fn cold_arm_pure_builders_whitelist() {
        for i in [
            RegIntrinsic::StringCopy,
            RegIntrinsic::StringFromBool,
            RegIntrinsic::StringFromFloat,
            RegIntrinsic::StringFromInt,
            RegIntrinsic::StringSlice,
            RegIntrinsic::StringPadLeft,
            RegIntrinsic::BytesFromString,
            RegIntrinsic::BytesSlice,
        ] {
            assert!(intrinsic_descriptor(i).cold_arm_pure_builder, "{:?}", i);
        }
        // Not on the builder whitelist: queries, combinators, opaque allocators / mutators.
        for i in [
            RegIntrinsic::StringLen,
            RegIntrinsic::OptionMap,
            RegIntrinsic::ListContains,
        ] {
            assert!(!intrinsic_descriptor(i).cold_arm_pure_builder, "{:?}", i);
        }
        // Pure first-order scalar READERS are cold-arm eligible via the separate
        // `cold_arm_pure_reader` flag (NOT builders — they allocate nothing).
        for i in [
            RegIntrinsic::StringCount,
            RegIntrinsic::StringContains,
            RegIntrinsic::StringIndexOf,
            RegIntrinsic::StringStartsWith,
        ] {
            let d = intrinsic_descriptor(i);
            assert!(d.cold_arm_pure_reader, "reader {:?}", i);
            assert!(!d.cold_arm_pure_builder, "reader is not a builder {:?}", i);
        }
        // Higher-order combinators carry neither the builder nor the reader flag (they are
        // not value producers/queries); they are cold-arm eligible via `combinator_kind`
        // instead — a combinator + its closure run only on the interpreter replay after a
        // cold-arm bail, so any closure effect happens exactly as without the JIT.
        for i in [RegIntrinsic::OptionMap, RegIntrinsic::ResultAndThen] {
            let d = intrinsic_descriptor(i);
            assert!(
                !d.cold_arm_pure_reader && !d.cold_arm_pure_builder,
                "{:?}",
                i
            );
            assert!(d.combinator_kind.is_some(), "combinator {:?}", i);
        }
    }
}

#[cfg(test)]
mod register_window_tests {
    #[cfg(feature = "native-jit")]
    use crate::reg_vm::tier::native_scalar_callee_pending_on_branch_profile;

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
            native_signatures: HashMap::new(),
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
        let big: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
                VmValue::Int(0);
                4096
            ])));
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
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    #[cfg(feature = "native-jit")]
    fn native_test_layout(name: &str, fields: &[&str]) -> Rc<crate::vm_value::TypeLayout> {
        intern_layout(
            Rc::from(name),
            fields.iter().map(|field| Rc::from(*field)).collect(),
        )
    }

    #[cfg(feature = "native-jit")]
    fn native_test_unit(functions: Vec<RegFunction>) -> RegUnit {
        RegUnit {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        }
    }

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
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
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
features: local

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
    Log.write(message: read String.from_int(value: a + b))
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
features: local

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
    Log.write(message: read String.from_int(value: caller(g: read bag)))
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
    Log.write(message: read String.from_int(value: scan(chars: read chars, i: 1)))
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
features: local

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
    Log.write(message: read String.from_int(value: scan(xs: read xs, i: 0)))
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
    Log.write(message: read String.from_int(value: pick(opt: read Some(42))))
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
    Log.write(message: read String.from_int(value: measure(s: read "hello")))
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
features: local

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
    Log.write(message: read String.from_int(value: List.get<Int>(list: read xs, index: 0)))
    Log.write(message: read String.from_int(value: r))
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
features: local

fn fib(n: Int) -> Int {
    if n < 2 {
        return n
    }
    return fib(n: n - 1) + fib(n: n - 2)
}

fn main() -> Unit {
    let value = fib(n: 10)
    Log.write(message: read String.from_int(value: value))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("scalar-recursion.rss", source).expect("lowering should succeed");
        let output = executable
            .eval_main_with_args_jit(Vec::<String>::new())
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
    fn native_translation_lowers_flat_int_list_set_to_direct_write() {
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
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListGetIntDirect { .. })),
            "List.get<Int> on the same flat mutable list param should lower to direct read; jit code: {:#?}",
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
    fn native_aggregate_region_scalar_replacement_rejects_external_read() {
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

        assert!(
            native_scalar_replace_variants_in_region(&code, 5, 1, 4).is_none(),
            "variant SR must reject regions whose original aggregate is read after OSR exit",
        );
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
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
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
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
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
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
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
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
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
    fn native_translation_lowers_checked_json_field_int_payload() {
        let function = native_test_function(
            "json_field_int_hot",
            2,
            4,
            vec![
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::JsonFieldInt,
                    args: vec![0, 1],
                    dst: 2,
                },
                RegInstr::TryResult {
                    dst: 3,
                    src: 2,
                    cleanup: Vec::new(),
                },
                RegInstr::Return { src: 3 },
            ],
        );
        let unit = native_test_unit(vec![function]);
        let (jit, _, _, _, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("checked Json.field_int payload should translate");

        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::JsonFieldInt,
                    ..
                }
            )),
            "Json.field_int(...)? should lower to the checked native payload helper",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_lowers_string_literals_through_helper_table() {
        let function = native_test_function(
            "literal_hot",
            0,
            1,
            vec![
                RegInstr::LoadString {
                    dst: 0,
                    value: Rc::new("id".to_string()),
                },
                RegInstr::Return { src: 0 },
            ],
        );
        let unit = native_test_unit(vec![function]);
        let (jit, ret, _, literals, _) = translate_to_native_jit(&unit, unit.functions[0].as_ref())
            .expect("string literal return should translate");

        assert_eq!(ret, NativeTy::Handle);
        assert_eq!(literals.len(), 1);
        assert_eq!(&*literals[0], "id");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringLiteral,
                    args,
                    ..
                } if args == &vec![vm_jit::HostArg::ImmI64(0)]
            )),
            "LoadString should lower to StringLiteral with a per-function literal id",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translation_preserves_handle_return_compiled_call() {
        let source = "\
fn make_text(value: Int) -> String {
    return String.from_int(value: value + 100)
}

fn text_len(value: Int) -> Int {
    let text = make_text(value: read value)
    return String.len(value: read text)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: text_len(value: read 23)))
    return Unit
}
";
        let executable = reg_vm_compile_source("native-handle-call-translate.rss", source)
            .expect("source compiles");
        let make = executable.unit.function_ids["make_text"];
        let text_len = executable.unit.function_ids["text_len"];
        let make_func = executable.unit.functions[make].as_ref();
        let text_len_func = executable.unit.functions[text_len].as_ref();
        let (child_jit, child_ret, child_params, _, _) =
            translate_to_native_jit(&executable.unit, make_func)
                .expect("handle-return child should translate");
        assert_eq!(child_ret, NativeTy::Handle);

        let mut module = vm_jit::NativeModule::new(jit_host_helpers()).expect("native module");
        let child_id = module.compile(&child_jit).expect("compile child");
        let call_ip = text_len_func
            .code
            .iter()
            .position(|instr| {
                matches!(
                    instr,
                    RegInstr::CallKnown {
                        function,
                        mut_args,
                        ..
                    } if *function == make && mut_args.is_empty()
                )
            })
            .expect("text_len should call make_text");
        let compiled = std::collections::HashMap::from([(
            call_ip,
            NativeCompiledCallee {
                id: child_id,
                ret_ty: child_ret,
                param_tys: child_params,
            },
        )]);
        let (parent_jit, parent_ret, _, _, _) = translate_to_native_jit_with_compiled_callees(
            &executable.unit,
            text_len_func,
            &compiled,
        )
        .expect("parent should preserve handle-return native call");
        assert_eq!(parent_ret, NativeTy::Int);
        assert!(
            parent_jit
                .code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::CallNative { .. })),
            "parent IR should contain CallNative, got {:?}",
            parent_jit.code
        );
        assert!(
            parent_jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringLen,
                    ..
                }
            )),
            "parent IR should consume the child handle through StringLen, got {:?}",
            parent_jit.code
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_consumes_handle_return_compiled_call() {
        let source = "\
fn make_text(value: Int) -> String {
    return String.from_int(value: value + 100)
}

fn text_len(value: Int) -> Int {
    let text = make_text(value: read value)
    return String.len(value: read text)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: text_len(value: read 23)))
    return Unit
}
";
        let executable = reg_vm_compile_source("native-handle-call-direct.rss", source)
            .expect("source compiles");
        let text_len = executable.unit.function_ids["text_len"];
        let func = Rc::clone(&executable.unit.functions[text_len]);
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(23));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(3)) => {}
            NativeAttempt::Completed(value) => {
                panic!("text_len completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "text_len unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "text_len unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native handle-return edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_passes_float_args_and_return() {
        let source = "\
fn scale(value: Float, factor: Float) -> Float {
    return value * factor
}

fn parent(seed: Float) -> Float {
    let scaled = scale(value: read seed, factor: read 2.0)
    return scaled + 1.25
}

fn main() -> Unit {
    Log.write(message: read Float.to_string(value: read parent(seed: read 3.5)))
    return Unit
}
";
        let executable =
            reg_vm_compile_source("native-float-call-direct.rss", source).expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Float(3.5));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Float(value))
                if (value - 8.25).abs() < f64::EPSILON => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native float edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_passes_bool_args_and_return() {
        let source = "\
fn matches(flag: Bool, value: Int) -> Bool {
    let over = value > 10
    return flag == over
}

fn parent(seed: Int) -> Int {
    let ok = matches(flag: read true, value: read seed)
    if ok {
        return seed + 1
    }
    return 0
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: parent(seed: read 11)))
    return Unit
}
";
        let executable =
            reg_vm_compile_source("native-bool-call-direct.rss", source).expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(11));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(12)) => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native bool edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_passes_flat_int_arg_to_compiled_call() {
        let source = "\
features: local

fn read_at(values: read List<Int>, index: Int) -> Int {
    return List.get<Int>(list: read values, index: index) + List.len<Int>(list: read values)
}

fn parent(values: read List<Int>, index: Int) -> Int {
    return read_at(values: read values, index: read index) + List.len<Int>(list: read values)
}

fn main() -> Unit {
    local values = List.new<Int>()
    List.push<Int>(list: mut values, value: read 5)
    List.push<Int>(list: mut values, value: read 7)
    List.push<Int>(list: mut values, value: read 11)
    Log.write(message: read String.from_int(value: parent(values: read values, index: read 1)))
    return Unit
}
";
        let executable = reg_vm_compile_source("native-flat-int-param-call-direct.rss", source)
            .expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let values = Rc::new(RefCell::new(TypedVec::Ints(vec![5, 7, 11])));
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::clone(&values)));
        vm.set_reg(1, VmValue::Int(1));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(13)) => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native flat-list edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_passes_flat_float_arg_to_compiled_call() {
        let source = "\
features: local

fn read_at(values: read List<Float>, index: Int) -> Float {
    return List.get<Float>(list: read values, index: index) + Int.to_float(value: read List.len<Float>(list: read values))
}

fn parent(values: read List<Float>, index: Int) -> Float {
    return read_at(values: read values, index: read index) + List.get<Float>(list: read values, index: 0)
}

fn main() -> Unit {
    local values = List.new<Float>()
    List.push<Float>(list: mut values, value: read 1.25)
    List.push<Float>(list: mut values, value: read 2.5)
    List.push<Float>(list: mut values, value: read 3.75)
    Log.write(message: read Float.to_string(value: read parent(values: read values, index: read 1)))
    return Unit
}
";
        let executable = reg_vm_compile_source("native-flat-float-param-call-direct.rss", source)
            .expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let values = Rc::new(RefCell::new(TypedVec::Floats(vec![1.25, 2.5, 3.75])));
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::clone(&values)));
        vm.set_reg(1, VmValue::Int(1));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Float(value))
                if (value - 6.75).abs() < f64::EPSILON => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native flat-float-list edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_passes_flat_int_mut_arg_to_compiled_call() {
        let source = "\
features: local

fn write_at(values: mut List<Int>, index: Int, value: Int) -> Int {
    List.set<Int>(list: mut values, index: index, value: read value)
    return List.get<Int>(list: read values, index: index)
}

fn parent(values: mut List<Int>, index: Int, value: Int) -> Int {
    let written = write_at(values: mut values, index: read index, value: read value)
    return written + List.get<Int>(list: read values, index: index)
}

fn main() -> Unit {
    local values = List.new<Int>()
    List.push<Int>(list: mut values, value: read 5)
    List.push<Int>(list: mut values, value: read 7)
    List.push<Int>(list: mut values, value: read 11)
    Log.write(message: read String.from_int(value: parent(values: mut values, index: read 1, value: read 42)))
    return Unit
}
";
        let executable = reg_vm_compile_source("native-flat-int-mut-param-call-direct.rss", source)
            .expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let values = Rc::new(RefCell::new(TypedVec::Ints(vec![5, 7, 11])));
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::List(Rc::clone(&values)));
        vm.set_reg(1, VmValue::Int(1));
        vm.set_reg(2, VmValue::Int(42));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(84)) => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        assert!(
            matches!(&*values.borrow(), TypedVec::Ints(items) if items == &[5, 42, 11]),
            "native child write should commit to the original list",
        );
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native flat mutable list edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_passes_handle_arg_to_compiled_call() {
        let source = "\
fn len_text(text: read String) -> Int {
    return String.len(value: read text)
}

fn parent(value: Int) -> Int {
    let text = String.from_int(value: value + 100)
    return len_text(text: read text)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: parent(value: read 23)))
    return Unit
}
";
        let executable = reg_vm_compile_source("native-handle-param-call-direct.rss", source)
            .expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(23));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(3)) => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native handle-param edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_direct_dispatch_invokes_mut_handle_compiled_call() {
        let source = "\
struct Box {
    value: Int
}

fn set_box(item: mut Box, value: Int) -> Int {
    item.value = value
    return item.value
}

fn parent(item: mut Box, value: Int) -> Int {
    let result = set_box(item: mut item, value: read value)
    return result
}

fn main() -> Unit {
    return Unit
}
";
        let executable = reg_vm_compile_source("native-mut-handle-call-direct.rss", source)
            .expect("source compiles");
        let parent = executable.unit.function_ids["parent"];
        let func = Rc::clone(&executable.unit.functions[parent]);
        let layout = native_test_layout("Box", &["value"]);
        let boxed = VmValue::Struct(Rc::new(VmStruct::with_layout(
            layout,
            vec![VmValue::Int(1)],
        )));
        let mut vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            HashMap::new(),
        );
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, boxed);
        vm.set_reg(1, VmValue::Int(99));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(99)) => {}
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed => {
                panic!(
                    "parent unexpectedly resumed, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
            NativeAttempt::Fallback => {
                panic!(
                    "parent unexpectedly fell back, stats={:?}",
                    vm.native.as_ref().expect("native").stats
                )
            }
        }
        let VmValue::Struct(updated) = vm.reg(0) else {
            panic!("expected updated Box in reg 0, got {:?}", vm.reg(0));
        };
        assert_eq!(updated.fields.first(), Some(&VmValue::Int(99)));
        let stats = &vm.native.as_ref().expect("native").stats;
        assert!(
            stats.native_call_edges >= 1,
            "parent should compile with a native-to-native mut-handle edge, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_region_rewrite_handles_source_json_field_loop() {
        let source = r#"
fn hot(profile: read JsonValue, limit: Int) -> Result<Int, JsonError> {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + Json.field_int(value: read profile, name: read "id")?
        index = index + 1
    }
    return Ok(total)
}

fn main() -> Result<Unit, JsonError> {
    let doc = Json.parse(text: read "{\"profile\":{\"id\":41}}")?
    let profile = Json.field(value: read doc, name: read "profile")?
    let total = hot(profile: read profile, limit: 10)?
    Log.write(message: read String.from_int(value: total))
    return Ok(Unit)
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let func = unit.functions[hot].as_ref();
        let lp = detect_single_natural_loop(&func.code).expect("hot loop should be detected");
        let (code, n_regs, _) = native_lower_checked_payload_intrinsics_in_region(
            &func.code, func.regs, lp.header, lp.exit,
        )
        .expect("checked JSON rewrite should run");

        assert!(
            code.iter().any(|instr| matches!(
                instr,
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::JsonFieldIntOk,
                    ..
                }
            )),
            "source Json.field_int(...)? loop should rewrite to JsonFieldIntOk",
        );

        let lp = detect_single_natural_loop(&code).expect("rewritten hot loop should be detected");
        let (jit, _, _, _, _, _, _) =
            translate_osr_loop(&code, n_regs, func.params, func.captures, lp)
                .expect("rewritten JSON field loop should translate to OSR native IR");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::JsonFieldInt,
                    ..
                }
            )),
            "OSR JSON field loop should memoize invariant JsonFieldInt; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::StringLiteral,
                    ..
                }
            )),
            "OSR JSON field loop should memoize loop-local string literals feeding invariant helpers; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_region_memoizes_invariant_json_parse_handle() {
        let source = r#"
fn hot(limit: Int) -> Result<Int, JsonError> {
    let text = "{\"id\":41}"
    let mut index = 0
    let mut total = 0
    while index < limit {
        let doc = Json.parse(text: read text)?
        total = total + Json.field_int(value: read doc, name: read "id")?
        index = index + 1
    }
    return Ok(total)
}

fn main() -> Result<Unit, JsonError> {
    let total = hot(limit: 10)?
    Log.write(message: read String.from_int(value: total))
    return Ok(Unit)
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let func = unit.functions[hot].as_ref();
        let lp = detect_single_natural_loop(&func.code).expect("hot loop should be detected");
        let (code, n_regs, _) = native_lower_checked_payload_intrinsics_in_region(
            &func.code, func.regs, lp.header, lp.exit,
        )
        .expect("checked JSON rewrite should run");

        assert!(
            code.iter().any(|instr| matches!(
                instr,
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::JsonParseOk,
                    ..
                }
            )),
            "source Json.parse(...)? loop should rewrite to JsonParseOk",
        );

        let lp = detect_single_natural_loop(&code).expect("rewritten hot loop should be detected");
        let (jit, _, _, _, _, _, _) =
            translate_osr_loop(&code, n_regs, func.params, func.captures, lp)
                .expect("rewritten JSON parse loop should translate to OSR native IR");
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::JsonParse,
                    ..
                }
            )),
            "OSR JSON parse loop should memoize invariant JsonParse; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::JsonFieldInt,
                    ..
                }
            )),
            "a memoized JsonParse handle should become invariant for dependent JsonFieldInt; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_preserves_heap_live_through_scalar_loop() {
        let source = r#"features: local

fn main() -> Unit {
    let mut q: Deque<Int> = Deque.new<Int>()
    Log.write(message: read String.from_int(value: Deque.len(deque: read q)))
    let mut xs: List<Int> = List.new<Int>()
    let mut i = 0
    while i < 1 {
        let mut tmp = 948
        i = i + 1
    }
    let ys: List<Int> = xs
    Log.write(message: read String.from_int(value: List.len(list: read ys)))
    Log.write(message: read String.from_int(value: 766))
    Log.write(message: read String.from_int(value: 146))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("osr-live-through-heap.rss", source).expect("compile");

        let interp = executable
            .eval_main_with_args(std::iter::empty::<&str>())
            .expect("interpreter run");
        let osr = executable
            .eval_main_with_args_native_osr(std::iter::empty::<&str>())
            .expect("native OSR run must preserve live-through heap slots");

        assert_eq!(osr.stdout, interp.stdout);
        assert_eq!(osr.stdout, "0\n0\n766\n146\n");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_enters_checked_json_payload_loop() {
        let source = r#"
fn hot(profile: read JsonValue, limit: Int) -> Result<Int, JsonError> {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + Json.field_int(value: read profile, name: read "id")?
        index = index + 1
    }
    return Ok(total)
}

fn main() -> Result<Unit, JsonError> {
    let doc = Json.parse(text: read "{\"profile\":{\"id\":41}}")?
    let profile = Json.field(value: read doc, name: read "profile")?
    let total = hot(profile: read profile, limit: 200)?
    Log.write(message: read String.from_int(value: total))
    return Ok(Unit)
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["hot"];
        let func = executable.unit.functions[hot].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("checked JSON payload loop should be eligible for OSR selection");

        let (out, stats) = executable
            .eval_main_with_args_native_osr_with_stats(std::iter::empty::<&str>())
            .expect("program should run");

        assert_eq!(out.stdout, "8200\n");
        assert!(
            stats.osr_entries > 0,
            "checked JSON payload loop should OSR-enter; candidate={candidate:?}; stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_region_memoizes_read_only_field_loads() {
        let source = r#"
struct Boxed {
    capacity: Int
    values: List<Int>
}

fn hot(box: read Boxed, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        total = total + box.capacity + List.len<Int>(list: read box.values)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    let values = List<Int>.new()
    List.push<Int>(list: mut values, value: 1)
    let box = Boxed(capacity: 16, values: values)
    let total = hot(box: read box, limit: 10)
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
        let hot = unit.function_ids["hot"];
        let func = unit.functions[hot].as_ref();
        let lp = detect_single_natural_loop(&func.code).expect("hot loop should be detected");
        let (jit, _, _, scalar_fields, _, _, _) =
            translate_osr_loop(&func.code, func.regs, func.params, func.captures, lp)
                .expect("read-only field loop should translate to OSR native IR");

        assert!(
            scalar_fields
                .iter()
                .any(|field| field.field_slot == 0 && !field.writeback),
            "read-only Int field load should become a scalar OSR field; scalar={:#?}; jit code={:#?}",
            scalar_fields,
            jit.code,
        );
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldInt,
                    ..
                } | vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::FieldInt,
                    ..
                }
            )),
            "read-only Int field load should avoid FieldInt helpers; jit code: {:#?}",
            jit.code,
        );
        assert!(
            jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::FieldHandle,
                    ..
                }
            )),
            "read-only handle field load should memoize when no in-loop store targets that slot; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_finds_later_native_loop_after_setup_loop() {
        let source = r#"
features: local

fn bench_size(default: Int) -> Int {
    let raw = Args.get_or_default(index: 0, default: read String.from_int(value: default))
    match String.parse_int(value: read raw) {
        Some(value) => {
            return value
        }
        None => {
            return default
        }
    }
}

fn main() -> Unit {
    let limit = bench_size(default: 40000)
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

    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
        let unit = RegUnit::lower(&hir).expect("lowering should succeed");
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
features: local

fn main() -> Unit {
    let limit = 64
    let mut index = 0
    local values = List<Int>.new()

    while index < limit {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }

    Log.write(message: read String.from_int(value: List.len<Int>(list: read values)))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
        );
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
    fn native_osr_enters_direct_async_call_await_loop() {
        let source = r#"
features: async

async fn step(value: Int) -> Result<Int, String> {
    return Ok(value + 1)
}

async fn main() -> Result<Unit, String> {
    let mut index = 0
    let mut total = 0
    while index < 2000 {
        total = await step(value: total)?
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
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
features: async

async fn step(value: Int) -> Option<Int> {
    return Some(value + 1)
}

async fn main() -> Option<Unit> {
    let mut index = 0
    let mut total = 0
    while index < 2000 {
        total = await step(value: total)?
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
features: local

fn hot(values: mut List<Int>, slot: Int, replacement: Int) -> Int {
    List.set<Int>(list: mut values, index: slot, value: read replacement)
    return List.get<Int>(list: read values, index: slot)
}

fn main() -> Unit {
    local values = List<Int>.new()
    List.push<Int>(list: mut values, value: read 1)
    List.push<Int>(list: mut values, value: read 2)
    List.push<Int>(list: mut values, value: read 3)

    let total = hot(values: mut values, slot: 1, replacement: 7) + List.get<Int>(list: read values, index: 1)
    Log.write(message: read String.from_int(value: total))
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
features: local

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

fn main() -> Unit {
    local values = List<Int>.new()
    let mut i = 0
    while i < 8 {
        List.push<Int>(list: mut values, value: read 0)
        i = i + 1
    }

    let total = hot(values: mut values, limit: 32) + List.get<Int>(list: read values, index: 7)
    Log.write(message: read String.from_int(value: total))
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
            jit.code
                .iter()
                .any(|instr| matches!(instr, vm_jit::JitInstr::ListGetIntDirect { .. })),
            "steady-state List.get should lower to direct flat read; jit={:#?}",
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
features: local

fn sum_len(seen: read SortedSet<Int>) -> Int {
    let mut i = 0
    let mut total = 0
    while i < SortedSet.len<Int>(set: read seen) {
        total = total + i
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    local seen = SortedSet.new<Int>()
    let mut i = 0
    while i < 32 {
        let _inserted = SortedSet.insert<Int>(set: mut seen, value: read i)
        i = i + 1
    }

    let total = sum_len(seen: read seen)
    Log.write(message: read String.from_int(value: total))
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
                vm_jit::JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ListLen,
                    ..
                }
            )),
            "SortedSet.len should lower through the existing ListLen host helper; code={:#?}",
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
features: local

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

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: empty_score + non_empty_score))
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
features: local

fn map_set_len_score(table: read Map<Int, Int>, set: read Set<Int>) -> Int {
    return (Map.len<Int, Int>(map: read table) * 10) + Set.len<Int>(set: read set)
}

fn main() -> Unit {
    local table = Map<Int, Int>.new()
    local set = Set.new<Int>()
    let empty_score = map_set_len_score(table: read table, set: read set)

    Map.insert<Int, Int>(map: mut table, key: read 1, value: read 2)
    Set.insert(set: mut set, value: read 3)
    let non_empty_score = map_set_len_score(table: read table, set: read set)

    Log.write(message: read String.from_int(value: empty_score + non_empty_score))
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
    fn native_translates_bytes_len_and_slice_helpers() {
        let source = r#"
features: local

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

fn main() -> Unit {
    let data = Bytes.from_string(value: read "abcdef")
    let total = bytes_slice_score(data: read data, reps: 3)
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let hot = executable.unit.function_ids["bytes_slice_score"];
        let func = executable.unit.functions[hot].as_ref();
        let (jit, _, _, _, _) = translate_to_native_jit(&executable.unit, func)
            .expect("Bytes.len/Bytes.slice helper function should translate to native IR");
        for expected in [vm_jit::HostHelper::BytesSlice, vm_jit::HostHelper::BytesLen] {
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
            "bytes_slice_score should run through whole-function native JIT; stats={stats:?}",
        );
        assert_eq!(out.stdout.trim(), "30");
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_translates_string_slice_helper() {
        let source = r#"
features: local

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

fn main() -> Unit {
    let total = string_slice_score(value: read "abcdef", reps: 3)
    Log.write(message: read String.from_int(value: total))
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
features: local

fn main() -> Unit {
    local table = SortedMap<Int, Int>.new()
    let mut i = 0
    while i < 32 {
        let value = i * 2
        SortedMap.insert<Int, Int>(map: mut table, key: read i, value: read value)
        i = i + 1
    }

    Log.write(message: read String.from_int(value: SortedMap.len<Int, Int>(map: read table)))
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        let executable =
            reg_vm_compile_source("test.rss", source).expect("lowering should succeed");
        let main = executable.unit.function_ids["main"];
        let func = executable.unit.functions[main].as_ref();
        let candidate = super::super::tier::select_osr_candidate_loop(&executable.unit, func)
            .expect("sorted-map get loop should be selected for OSR");
        translate_osr_loop(&func.code, func.regs, func.params, func.captures, candidate)
            .unwrap_or_else(|| {
                panic!(
                    "selected sorted-map get loop should translate to OSR native IR; region={:#?}",
                    &func.code[candidate.header..candidate.exit],
                )
            });
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

        assert_eq!(
            rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 1),
            0
        );
        assert_eq!(rss_jit_map_get_match_found(JitCallCtx::active_token()), 1);
        assert_eq!(
            rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 2),
            22
        );
        assert_eq!(rss_jit_map_get_match_found(JitCallCtx::active_token()), 1);
        assert_eq!(
            rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 3),
            0
        );
        assert_eq!(rss_jit_map_get_match_found(JitCallCtx::active_token()), 0);
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

        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 1),
            10
        );
        assert_eq!(rss_jit_sorted_map_get_found(JitCallCtx::active_token()), 1);
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 3),
            30
        );
        assert_eq!(rss_jit_sorted_map_get_found(JitCallCtx::active_token()), 1);
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 7),
            70
        );
        assert_eq!(rss_jit_sorted_map_get_found(JitCallCtx::active_token()), 1);

        // Non-sequential access must still fall back to the binary-search path.
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 3),
            30
        );
        assert_eq!(rss_jit_sorted_map_get_found(JitCallCtx::active_token()), 1);
        assert_eq!(
            rss_jit_sorted_map_get_int(JitCallCtx::active_token(), 0, 4),
            0
        );
        assert_eq!(rss_jit_sorted_map_get_found(JitCallCtx::active_token()), 0);

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
            assert_eq!(
                rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 1),
                10
            );
            assert_eq!(rss_jit_map_get_match_found(JitCallCtx::active_token()), 1);
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
            assert_eq!(
                rss_jit_map_get_match_int(JitCallCtx::active_token(), 0, 1),
                30,
                "handle 0 in a new native attempt must resolve the new heap arg, not the cached prior map",
            );
            assert_eq!(rss_jit_map_get_match_found(JitCallCtx::active_token()), 1);
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
features: local

fn main() -> Unit {
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

    Log.write(message: read String.from_int(value: total))
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
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
        );
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

fn main() -> Unit {
    let mut box = Box(value: 0)
    let mut i = 0
    let mut total = 0
    while i < 32 {
        box.value = i
        total = total + box.value
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
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

fn main() -> Unit {
    let mut box = Box(value: 0)
    let mut i = 0
    while i < 32 {
        box.value = i
        i = i + 1
    }
    Log.write(message: read String.from_int(value: box.value))
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

fn main() -> Unit {
    let mut box = Box(value: 0)
    let _value = bump(box: mut box)
    Log.write(message: read String.from_int(value: box.value))
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

fn main() -> Unit {
    let mut box = Box(value: 0)
    bump_loop(box: mut box, limit: 32)
    Log.write(message: read String.from_int(value: box.value))
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
features: local

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

fn main() -> Unit {
    let value = hot(x: 1)
    Log.write(message: read String.from_int(value: value))
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
        let (code, n_regs, _) =
            native_scalar_replace_variants_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("variant SR should accept inlined hot loop");
        let lp = detect_natural_loops(&code)
            .into_iter()
            .next()
            .expect("loop should remain after variant SR");
        let (code, n_regs, _) =
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
        let (code, n_regs, _) =
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
        let vm = RegVm::new(
            Rc::clone(&executable.unit),
            Vec::<String>::new(),
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
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
        let (code, n_regs, _) =
            native_scalar_replace_variants_in_region(&code, n_regs, lp.header, lp.exit)
                .expect("variant SR should accept selfhost mailbox hot loop");
        let lp =
            detect_natural_loop_at(&code, lp.header).expect("loop should remain after variant SR");
        let (code, n_regs, _) =
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

    #[cfg(feature = "native-jit")]
    #[test]
    fn tier0_jit_pure_leaf_call_avoids_frame_push() {
        let callee = native_test_function(
            "plus_one",
            1,
            3,
            vec![
                RegInstr::LoadInt { dst: 1, value: 1 },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::Return { src: 2 },
            ],
        );
        let caller = native_test_function(
            "caller",
            1,
            2,
            vec![
                RegInstr::CallKnown {
                    dst: 1,
                    function: 0,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let unit = Rc::new(native_test_unit(vec![callee, caller]));
        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.set_limits(VmLimits {
            max_depth: 0,
            ..VmLimits::default()
        });
        let caller = Rc::clone(&unit.functions[1]);
        vm.prepare_frame(0, caller.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(41));

        let result = vm
            .run_jit(&unit, caller.as_ref(), 0)
            .expect("pure leaf call should not push a VM frame");

        assert_eq!(result, VmValue::Int(42));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_prefilter_keeps_scalar_replaceable_aggregates_for_translation() {
        let variant_layout = native_test_layout("Boxed", &["value"]);
        let struct_layout = native_test_layout("Pair", &["a", "b"]);
        let functions = vec![
            native_test_function(
                "variant_hot",
                1,
                5,
                vec![
                    RegInstr::MakeVariant {
                        dst: 1,
                        layout: Rc::clone(&variant_layout),
                        fields: vec![("value".to_string(), 0)],
                    },
                    RegInstr::MatchVariant {
                        src: 1,
                        expected: "Boxed".to_string(),
                        match_ip: 2,
                        else_ip: 4,
                    },
                    RegInstr::UnwrapVariantValue {
                        dst: 2,
                        src: 1,
                        expected: "Boxed".to_string(),
                    },
                    RegInstr::Return { src: 2 },
                    RegInstr::Return { src: 0 },
                ],
            ),
            native_test_function(
                "struct_hot",
                1,
                5,
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
                    RegInstr::Return { src: 3 },
                ],
            ),
        ];

        mark_predictably_native_ineligible(&functions);

        assert_eq!(
            functions[0].native_status.get(),
            0,
            "scalar-replaceable variants must reach the real native translator",
        );
        assert_eq!(
            functions[1].native_status.get(),
            0,
            "scalar-replaceable structs must reach the real native translator",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_prefilter_keeps_sinkable_closures_for_translation() {
        let callee = native_test_function(
            "mapper",
            1,
            3,
            vec![
                RegInstr::LoadInt { dst: 1, value: 1 },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
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
        let functions = vec![callee, caller];

        mark_predictably_native_ineligible(&functions);

        assert_eq!(
            functions[1].native_status.get(),
            0,
            "sinkable local closures must reach the real native translator",
        );
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
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    /// Predict-and-skip bail: a function that PASSES the
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
            if func.native_status.get() == NATIVE_STATUS_NOT_ELIGIBLE && first_demoted_at.is_none()
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
            !vm.native
                .as_ref()
                .unwrap()
                .cache
                .contains_key(&(&func as *const RegFunction as usize)),
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
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
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

    /// B2 (FIXED by J0.1 heap-aware deopt state maps): the reg VM binds params as
    /// locals, so `n = n + 1` rewrites the param register. A native scalar function
    /// that REASSIGNS a param still live at a safepoint must, on precise deopt, restore
    /// the param to its native-computed value. The deopt state map distinguishes
    /// reconstructible scalars (`Int`/`Float`) from heap refs (`Handle`/`FlatInt`/
    /// `FlatFloat`): `decode_deopt_live` drops the latter (the frame already holds
    /// their `VmValue`), so `restore_native_deopt_live_regs` can restore ALL scalar
    /// regs — params included — without the old `< n_params` skip that lost reassigned
    /// scalar params. (Skipping only `Handle` and not `FlatInt`/`FlatFloat` corrupts
    /// flat-buffer params — see `native_heap_reads`/`tv2_direct_flat_reads`.)
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_restores_reassigned_scalar_param() {
        let big: i64 = 4_000_000_000;
        // reg 0 = param x (runtime `big`); reg 0 = x + 1 (REASSIGN, live past the
        // safepoint); reg 2 = reg0 * reg0 -> runtime overflow guard bail; reg 2 =
        // reg0 + reg2 keeps reg 0 live at the MulInt safepoint.
        let func = Rc::new(native_test_function(
            "reassign",
            1,
            3,
            vec![
                RegInstr::LoadInt { dst: 1, value: 1 },
                RegInstr::AddInt {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::MulInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 0,
                },
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 2,
                },
                RegInstr::Return { src: 2 },
            ],
        ));
        let mut vm = empty_vm();
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );
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
            "the overflow guard must precisely resume",
        );
        assert_eq!(
            *vm.reg(0),
            VmValue::Int(big + 1),
            "a reassigned scalar param must be restored to its native-computed value \
             (big + 1), not the stale call-time `big`",
        );
    }

    /// Staged native-call ABI: when a scalar native callee deopts, RSScript must
    /// reconstruct the logical caller+callee interpreter frames instead of
    /// resuming at the caller's `CallKnown` site or falling back from the top.
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_with_child_native_frame_resumes_child_safepoint() {
        let child = add_then_square_func();
        let caller = native_test_function(
            "caller",
            1,
            2,
            vec![
                RegInstr::CallKnown {
                    dst: 1,
                    function: 0,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let unit = Rc::new(native_test_unit(vec![child, caller]));
        let caller = Rc::clone(&unit.functions[1]);

        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );

        let big: i64 = 4_000_000_000;
        vm.prepare_frame(0, caller.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&caller),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        let outcome = vm.try_native(&caller, 0);
        assert!(
            matches!(outcome, NativeAttempt::Resumed),
            "nested native callee deopt must reconstruct the interpreter frame chain",
        );
        assert_eq!(
            vm.frames.len(),
            2,
            "resume should leave caller suspended below the deopted child frame",
        );
        assert_eq!(
            vm.frames[0].ip, 1,
            "caller frame should resume after its CallKnown once the child returns",
        );
        assert_eq!(
            vm.frames[1].ip, 2,
            "child frame should resume at the overflowing MulInt safepoint",
        );
        assert_eq!(
            *vm.reg(caller.regs + 1),
            VmValue::Int(big + 1),
            "child non-param live register should be restored from the nested payload",
        );

        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.native_bails, 1);
        assert_eq!(
            stats.native_child_bails, 1,
            "nested callee deopt should be visible in native telemetry",
        );
        assert_eq!(
            stats.native_child_resumes, 1,
            "nested callee deopt should be counted as a frame-chain resume",
        );
        assert!(
            stats.native_call_edges >= 1,
            "caller should compile with a native-to-native edge, stats={stats:?}",
        );
    }

    /// Staged native-call ABI: a nested native call chain must preserve the full
    /// deopt frame chain, not just the first child frame. This pins the RSScript
    /// embedding of vm-jit's nested `DeoptFrame` payloads.
    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_with_nested_child_native_frames_resumes_leaf_safepoint() {
        let leaf = add_then_square_func();
        let middle = native_test_function(
            "middle",
            1,
            3,
            vec![
                RegInstr::CallKnown {
                    dst: 2,
                    function: 0,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 2 },
            ],
        );
        let top = native_test_function(
            "top",
            1,
            2,
            vec![
                RegInstr::CallKnown {
                    dst: 1,
                    function: 1,
                    args: vec![0],
                    mut_args: Vec::new(),
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let unit = Rc::new(native_test_unit(vec![leaf, middle, top]));
        let top = Rc::clone(&unit.functions[2]);

        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );

        let big: i64 = 4_000_000_000;
        vm.prepare_frame(0, top.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(big));
        vm.push_frame(Frame {
            func: Rc::clone(&top),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        let outcome = vm.try_native(&top, 0);
        assert!(
            matches!(outcome, NativeAttempt::Resumed),
            "nested native leaf deopt must reconstruct all interpreter frames",
        );
        assert_eq!(
            vm.frames.len(),
            3,
            "resume should leave top, middle, and deopted leaf frames",
        );
        assert_eq!(
            vm.frames[0].ip, 1,
            "top frame should resume after its CallKnown once middle returns",
        );
        assert_eq!(
            vm.frames[1].ip, 1,
            "middle frame should resume after its CallKnown once leaf returns",
        );
        assert_eq!(
            vm.frames[2].ip, 2,
            "leaf frame should resume at the overflowing MulInt safepoint",
        );

        let leaf_base = top.regs + unit.functions[1].regs;
        assert_eq!(
            *vm.reg(leaf_base + 1),
            VmValue::Int(big + 1),
            "leaf non-param live register should be restored from the nested payload",
        );

        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(stats.native_bails, 1);
        assert_eq!(
            stats.native_child_bails, 1,
            "nested leaf deopt should be visible in native telemetry",
        );
        assert_eq!(
            stats.native_child_resumes, 1,
            "nested leaf deopt should be counted as one frame-chain resume",
        );
        assert!(
            stats.native_call_edges >= 2 && stats.native_call_depth_max >= 2,
            "top should compile as a nested native-call chain, stats={stats:?}",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_deopt_every_env_parser_is_fail_closed() {
        assert!(!jit_native_deopt_every_from_env_value(None));
        assert!(!jit_native_deopt_every_from_env_value(Some("")));
        assert!(!jit_native_deopt_every_from_env_value(Some("0")));
        assert!(!jit_native_deopt_every_from_env_value(Some("false")));
        assert!(!jit_native_deopt_every_from_env_value(Some("FALSE")));
        assert!(jit_native_deopt_every_from_env_value(Some("1")));
        assert!(jit_native_deopt_every_from_env_value(Some("true")));
        assert!(jit_native_deopt_every_from_env_value(Some("yes")));
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_native_safepoints_match_interpreter_output() {
        let source = r#"
fn calc(x: Int) -> Int {
    let a = x + 1
    let b = a * 3
    let c = b / 2
    return c + a
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: calc(x: read 7)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        for safepoint in 1..=4 {
            let actual = reg_vm_eval_source_main_native_force_safepoint(
                "forced-safepoint.rss",
                source,
                std::iter::empty::<&str>(),
                safepoint,
            )
            .unwrap_or_else(|error| panic!("forced safepoint {safepoint} failed: {error:?}"));
            assert_eq!(
                actual.stdout, expected.stdout,
                "forced safepoint {safepoint} must preserve observable output",
            );
            assert_eq!(
                actual.value, expected.value,
                "forced safepoint {safepoint} must preserve main value",
            );
        }
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_all_native_safepoints_match_interpreter_output() {
        let source = r#"
fn calc(x: Int) -> Int {
    let a = x + 1
    let b = a * 3
    let c = b / 2
    return c + a
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: calc(x: read 7)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-all-safepoints.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        let actual = reg_vm_eval_source_main_native_force_all_safepoints(
            "forced-all-safepoints.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("native deopt-every-safepoint run");
        assert_eq!(
            actual.stdout, expected.stdout,
            "deopt-every-safepoint mode must preserve observable stdout",
        );
        assert_eq!(
            actual.value, expected.value,
            "deopt-every-safepoint mode must preserve main value",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_child_native_safepoint_resumes_and_returns_through_caller() {
        let source = r#"
fn child(x: Int) -> Int {
    let a = x + 1
    return a * a
}

fn caller(x: Int) -> Int {
    return child(x: read x) + 1
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: caller(x: read 4)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-child-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        let actual = reg_vm_eval_source_main_native_force_safepoint(
            "forced-child-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
            2,
        )
        .expect("native precise child safepoint");
        assert_eq!(actual.stdout, "26\n");
        assert_eq!(
            actual.stdout, expected.stdout,
            "child-frame precise deopt must resume, return into caller, and preserve stdout",
        );
        assert_eq!(
            actual.value, expected.value,
            "child-frame precise deopt must preserve main value",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn forced_all_native_safepoints_exercises_child_native_deopt() {
        let source = r#"
fn child(x: Int) -> Int {
    let a = x + 1
    return a * a
}

fn caller(x: Int) -> Int {
    return child(x: read x)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: caller(x: read 4)))
    return Unit
}
"#;
        let expected = reg_vm_eval_source_main_with_args(
            "forced-all-child-safepoint.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("interpreter");
        let executable =
            reg_vm_compile_source("forced-all-child-safepoint.rss", source).expect("compile");
        let (actual, stats) = executable
            .eval_main_with_args_native_inner(
                std::iter::empty::<&str>(),
                0,
                false,
                true,
                true,
                false,
                None,
                true,
            )
            .expect("native deopt-every child call");

        assert_eq!(actual.stdout, "25\n");
        assert_eq!(
            actual.stdout, expected.stdout,
            "deopt-every-safepoint mode must preserve stdout through a child native call",
        );
        assert_eq!(
            actual.value, expected.value,
            "deopt-every-safepoint mode must preserve main value through a child native call",
        );
        assert!(
            stats.native_call_edges >= 1 && stats.native_child_bails >= 1,
            "test must exercise a native-to-native child deopt, stats={stats:?}",
        );
        assert!(
            stats.native_child_resumes >= 1,
            "child deopt should resume through the native frame chain, stats={stats:?}",
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
    /// that handle unchanged. This is the original heap-result pass-through
    /// producer: NO allocation and NO mutation (native just returns a value it was
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
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }

    #[cfg(feature = "native-jit")]
    fn string_from_int_return_func() -> RegFunction {
        let code = vec![
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringFromInt,
                args: vec![0],
                dst: 1,
            },
            RegInstr::Return { src: 1 },
        ];
        RegFunction {
            name: "to_string".to_string(),
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

    #[cfg(feature = "native-jit")]
    fn string_from_int_len_func() -> RegFunction {
        let code = vec![
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringFromInt,
                args: vec![0],
                dst: 1,
            },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args: vec![1],
                dst: 2,
            },
            RegInstr::Return { src: 2 },
        ];
        RegFunction {
            name: "to_string_len".to_string(),
            params: 1,
            captures: 0,
            regs: 3,
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

    #[cfg(feature = "native-jit")]
    fn string_concat_len_func() -> RegFunction {
        let code = vec![
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringFromInt,
                args: vec![1],
                dst: 2,
            },
            RegInstr::StringConcat {
                dst: 3,
                left: 0,
                right: 2,
            },
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args: vec![3],
                dst: 4,
            },
            RegInstr::Return { src: 4 },
        ];
        RegFunction {
            name: "concat_len".to_string(),
            params: 2,
            captures: 0,
            regs: 5,
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

    /// Heap-result return ABI pass-through: a native function that returns a heap
    /// PARAMETER unchanged produces the interpreter-identical heap value, while the
    /// output table remains a clean per-call scratch area. Also asserts both tables
    /// are cleared on exit — the §7.2 invariant.
    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_result_passthrough_round_trips() {
        let mut vm = empty_vm();
        // threshold 0 => compile/attempt on the first call.
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(list_passthrough_func());

        // Distinct list value so identity is observable through the round-trip.
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
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
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "output table must be cleared on exit"
            )
        });
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_args.is_empty(),
                "input table must be cleared on exit"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_commits_clean_handle_result() {
        let mut tx = JitHeapTransactionGuard::begin();

        let handle = rss_jit_string_from_int(JitCallCtx::active_token(), 42);
        JIT_CALL_CTX.with(|ctx| {
            assert_eq!(
                ctx.borrow().heap_results.len(),
                1,
                "helper allocation should be staged before commit"
            )
        });

        match tx.commit_handle_with_writebacks(handle, &[]) {
            Some((VmValue::String(value), writebacks)) => {
                assert_eq!(&*value, "42");
                assert!(writebacks.is_empty());
            }
            Some((other, _)) => panic!("expected committed String result, got {other:?}"),
            None => panic!("expected staged handle to commit"),
        }
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "commit must clear staged heap results"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_abort_discards_staged_helper_result() {
        {
            let mut tx = JitHeapTransactionGuard::begin();

            let handle = rss_jit_string_from_int(JitCallCtx::active_token(), 7);
            assert!(
                handle < 0,
                "heap-producing helper should return an output-table handle"
            );
            JIT_CALL_CTX.with(|ctx| {
                assert_eq!(
                    ctx.borrow().heap_results.len(),
                    1,
                    "helper allocation should be staged before abort"
                )
            });

            tx.abort();
        }

        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "abort must discard staged helper allocations"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_helpers_fail_closed_outside_active_call_context() {
        {
            let _heap_guard = JitCallCtxGuard::enter();
            JitCallCtx::push_heap_arg(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                vec![VmValue::Int(1), VmValue::Int(2)],
            )))));
            assert_eq!(rss_jit_list_len(JitCallCtx::active_token(), 0), 2);
        }

        JIT_CALL_CTX.with(|ctx| {
            let ctx = ctx.borrow();
            assert_eq!(
                ctx.active_depth, 0,
                "test should be outside a native context"
            );
            assert!(
                ctx.heap_args.is_empty(),
                "dropping the call guard must clear heap inputs",
            );
        });

        assert_eq!(
            rss_jit_list_len(JitCallCtx::active_token(), 0),
            0,
            "a helper read outside an active native context must fail closed",
        );
        assert_eq!(
            rss_jit_string_from_int(JitCallCtx::active_token(), 7),
            0,
            "a helper allocation outside an active native context must not stage output",
        );
        assert_eq!(
            rss_jit_list_set_int(JitCallCtx::active_token(), 0, 0, 99),
            0,
            "a mutating helper outside an active native context must fail closed",
        );
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "inactive helper allocation must not leave staged heap output",
            );
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_helpers_reject_wrong_active_context_token() {
        let _heap_guard = JitCallCtxGuard::enter();
        let list = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ])));
        JitCallCtx::push_heap_arg(VmValue::List(Rc::clone(&list)));

        let token = JitCallCtx::active_token();
        assert_ne!(token, 0);
        assert_eq!(rss_jit_list_len(token, 0), 2);

        let wrong_token = token.wrapping_add(1).max(1);
        assert_ne!(wrong_token, token);
        assert_eq!(
            rss_jit_list_len(wrong_token, 0),
            0,
            "a helper read with the wrong active token must fail closed",
        );
        assert_eq!(
            rss_jit_list_set_int(wrong_token, 0, 0, 99),
            0,
            "a mutating helper with the wrong active token must fail closed",
        );
        assert_eq!(
            list.borrow().get(0),
            Some(VmValue::Int(1)),
            "wrong-token mutation must not reach the list",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_abort_restores_list_set_int_write() {
        let mut values = Vec::with_capacity(32);
        values.extend([1, 2, 3]);
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::Ints(values)));
        let original_capacity = match &*list.borrow() {
            TypedVec::Ints(values) => values.capacity(),
            _ => unreachable!(),
        };
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(VmValue::List(Rc::clone(&list)));

        let mut tx = JitHeapTransactionGuard::begin();
        assert_eq!(
            rss_jit_list_set_int(JitCallCtx::active_token(), 0, 1, 99),
            0
        );
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(99)),
            "native helper should mutate during the transaction"
        );

        tx.abort();
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(2)),
            "abort should restore the pre-native list contents"
        );
        let mut restored = list.borrow_mut();
        let restored_capacity = match &*restored {
            TypedVec::Ints(values) => values.capacity(),
            _ => unreachable!(),
        };
        assert_eq!(restored_capacity, original_capacity);
        assert_eq!(
            restored.checked_push_accounted(VmValue::Int(4)),
            Ok(0),
            "interpreter replay must retain spare capacity and avoid a false growth charge",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_snapshot_refuses_inactive_call_context() {
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ])));

        JIT_CALL_CTX.with(|ctx| {
            assert_eq!(
                ctx.borrow().active_depth,
                0,
                "test should start outside a native context"
            );
        });
        assert!(
            !jit_snapshot_list_before_write(0, &list),
            "snapshot registration must fail closed outside a native call frame",
        );
        JIT_HEAP_WRITE_UNDO.with(|undo| {
            assert!(
                undo.borrow().is_empty(),
                "inactive snapshot attempts must not create undo entries",
            );
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_heap_transaction_abort_restores_direct_flat_int_write() {
        let list: Rc<RefCell<TypedVec>> = Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
            VmValue::Int(3),
        ])));

        let mut tx = JitHeapTransactionGuard::begin();
        assert!(
            jit_snapshot_list_before_write(0, &list),
            "direct flat writes must be journaled inside a native transaction",
        );
        {
            let mut borrowed = list.borrow_mut();
            let slice = borrowed
                .as_ints_mut_slice()
                .expect("test list should use flat Int storage");
            assert_eq!(slice.len(), 3);
            slice[1] = 99;
        }
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(99)),
            "direct native write should mutate during the transaction"
        );

        tx.abort();
        assert_eq!(
            list.borrow().get(1),
            Some(VmValue::Int(2)),
            "abort should restore direct flat-list writes"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn precise_deopt_after_native_heap_write_falls_back_from_top() {
        let mut vm = empty_vm();
        vm.native = Some(
            NativeState::new_with_opt(0, false, true, false, true, false, false)
                .expect("native module"),
        );
        let func = Rc::new(native_test_function(
            "write_then_overflow",
            1,
            5,
            vec![
                RegInstr::LoadInt { dst: 1, value: 0 },
                RegInstr::LoadInt { dst: 2, value: 99 },
                RegInstr::ListSet {
                    dst: 3,
                    list: 0,
                    index: 1,
                    value: 2,
                },
                RegInstr::LoadInt {
                    dst: 4,
                    value: i64::MAX,
                },
                RegInstr::AddInt {
                    dst: 4,
                    lhs: 4,
                    rhs: 2,
                },
                RegInstr::Return { src: 4 },
            ],
        ));
        let list: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(1)])));

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
            "a guard bail after a native heap write must re-run from the top, not precise-resume",
        );
        assert_eq!(
            vm.frames.last().expect("frame").ip,
            0,
            "fallback path must leave the interpreter at the function entry",
        );
        assert_eq!(
            list.borrow().get(0),
            Some(VmValue::Int(1)),
            "heap transaction abort must restore the native write before fallback",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn flat_int_mut_alias_detects_list_hidden_inside_heap_input() {
        let list: Rc<RefCell<TypedVec>> =
            Rc::new(RefCell::new(TypedVec::from_values(vec![VmValue::Int(1)])));
        let layout = native_test_layout("Box", &["items"]);
        let heap_input = VmValue::Struct(Rc::new(VmStruct::with_layout(
            layout,
            vec![VmValue::List(Rc::clone(&list))],
        )));
        let _heap_guard = JitCallCtxGuard::enter();
        JitCallCtx::push_heap_arg(heap_input);

        assert!(
            jit_heap_inputs_alias_flat_mut(&[(0, 0)], &[Rc::clone(&list)]),
            "a handle input containing the same list Rc as a FlatIntMut arg must force fallback",
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_string_from_int_return_allocates_heap_result() {
        let mut vm = empty_vm();
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(string_from_int_return_func());

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(42));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::String(value)) => assert_eq!(&*value, "42"),
            NativeAttempt::Completed(_) => panic!("expected native String(\"42\") completion"),
            NativeAttempt::Resumed => panic!("expected native completion, got Resumed"),
            NativeAttempt::Fallback => panic!("expected native completion, got Fallback"),
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.translated, 1,
            "function should translate to native IR"
        );
        assert_eq!(
            stats.native_calls, 1,
            "function should complete in native code"
        );
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
                "output table must be cleared after materialization"
            )
        });
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_string_from_int_handle_feeds_string_len() {
        let mut vm = empty_vm();
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(string_from_int_len_func());

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::Int(12345));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(value)) => assert_eq!(value, 5),
            NativeAttempt::Completed(_) => panic!("expected native Int length completion"),
            NativeAttempt::Resumed => panic!("expected native completion, got Resumed"),
            NativeAttempt::Fallback => panic!("expected native completion, got Fallback"),
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.translated, 1,
            "function should translate to native IR"
        );
        assert_eq!(
            stats.native_calls, 1,
            "function should complete in native code"
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_string_concat_handle_feeds_string_len() {
        let mut vm = empty_vm();
        vm.native = Some(NativeState::new(0, false, true).expect("native module"));
        let func = Rc::new(string_concat_len_func());

        vm.prepare_frame(0, func.regs).expect("frame");
        vm.set_reg(0, VmValue::string("item-"));
        vm.set_reg(1, VmValue::Int(42));
        vm.push_frame(Frame {
            func: Rc::clone(&func),
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })
        .expect("push frame");

        match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(value)) => assert_eq!(value, 7),
            NativeAttempt::Completed(_) => panic!("expected native Int length completion"),
            NativeAttempt::Resumed => panic!("expected native completion, got Resumed"),
            NativeAttempt::Fallback => panic!("expected native completion, got Fallback"),
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.translated, 1,
            "function should translate to native IR"
        );
        assert_eq!(
            stats.native_calls, 1,
            "function should complete in native code"
        );
        assert_eq!(
            stats.native_bails, 0,
            "concat helper should not force a native bail"
        );
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
        JIT_CALL_CTX.with(|ctx| {
            assert!(
                ctx.borrow().heap_results.is_empty(),
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
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        };
        let mut vm = RegVm::new(Rc::new(unit), Vec::new(), HashMap::new());

        let a = vm.cached_noncapturing_closure(0);
        let b = vm.cached_noncapturing_closure(0);
        assert!(
            Rc::ptr_eq(&a, &b),
            "non-capturing closures of the same function must share one cached Rc",
        );
        assert!(
            a.captures.is_empty(),
            "cached closure must be non-capturing"
        );
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
    Log.write(message: read String.from_int(value: total))
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
