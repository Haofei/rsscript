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
    fn native_translation_classifies_scalar_alias_signatures() {
        let source = r#"
type Real = Float

fn add(left: Real, right: Real) -> Real {
    return left + right
}
"#;
        let executable =
            reg_vm_compile_source("native-scalar-alias.rss", source).expect("source compiles");
        let signature = &executable.unit.native_signatures["add"];
        assert_eq!(signature.params, vec!["Float", "Float"]);
        assert_eq!(signature.return_type.as_deref(), Some("Float"));

        let add = executable.unit.function_ids["add"];
        let (_, ret, params, _, _) =
            translate_to_native_jit(&executable.unit, executable.unit.functions[add].as_ref())
                .expect("scalar alias function should translate");
        assert_eq!(params, vec![NativeTy::Float, NativeTy::Float]);
        assert_eq!(ret, NativeTy::Float);
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
            tail_calls: 0,
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
            tail_calls: 0,
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
    fn native_float_comparison_branches_cover_ieee_edges_without_fallback() {
        let source = r#"
fn lt(left: Float, right: Float) -> Int { if left < right { return 1 } return 0 }
fn le(left: Float, right: Float) -> Int { if left <= right { return 1 } return 0 }
fn gt(left: Float, right: Float) -> Int { if left > right { return 1 } return 0 }
fn ge(left: Float, right: Float) -> Int { if left >= right { return 1 } return 0 }
fn eq(left: Float, right: Float) -> Int { if left == right { return 1 } return 0 }
fn ne(left: Float, right: Float) -> Int { if left != right { return 1 } return 0 }

fn main() -> Unit { return Unit }
"#;
        let executable = reg_vm_compile_source("native-float-compare-branches.rss", source)
            .expect("source compiles");
        let run = |name: &str, left: f64, right: f64| {
            let function_id = executable.unit.function_ids[name];
            let func = Rc::clone(&executable.unit.functions[function_id]);
            let mut vm = RegVm::new(
                Rc::clone(&executable.unit),
                Vec::<String>::new(),
                HashMap::new(),
            );
            vm.native = Some(NativeState::new(0, false, true).expect("native module"));
            vm.prepare_frame(0, func.regs).expect("frame");
            vm.set_reg(0, VmValue::Float(left));
            vm.set_reg(1, VmValue::Float(right));
            vm.push_frame(Frame {
                func: Rc::clone(&func),
                ip: 0,
                base: 0,
                ret_dst: usize::MAX,
                mut_writeback: Vec::new(),
                tail_calls: 0,
            })
            .expect("push frame");
            match vm.try_native(&func, 0) {
                NativeAttempt::Completed(VmValue::Int(value)) => value,
                NativeAttempt::Completed(value) => {
                    panic!("{name}({left:?}, {right:?}) returned {value:?}")
                }
                NativeAttempt::Resumed => {
                    panic!("{name}({left:?}, {right:?}) resumed instead of completing")
                }
                NativeAttempt::Fallback => {
                    panic!("{name}({left:?}, {right:?}) fell back from native execution")
                }
            }
        };
        let values = [
            (-1.0, 2.0),
            (2.0, -1.0),
            (3.0, 3.0),
            (0.0, -0.0),
            (f64::NEG_INFINITY, f64::INFINITY),
            (f64::NAN, 1.0),
            (1.0, f64::NAN),
            (f64::NAN, f64::NAN),
        ];
        for (left, right) in values {
            for (name, expected) in [
                ("lt", left < right),
                ("le", left <= right),
                ("gt", left > right),
                ("ge", left >= right),
                ("eq", left == right),
                ("ne", left != right),
            ] {
                assert_eq!(
                    run(name, left, right),
                    i64::from(expected),
                    "{name}({left:?}, {right:?})"
                );
            }
        }
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
            tail_calls: 0,
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
            tail_calls: 0,
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
            tail_calls: 0,
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
            tail_calls: 0,
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
            tail_calls: 0,
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
    fn native_direct_dispatch_rejects_mut_handle_compiled_call() {
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
            tail_calls: 0,
        })
        .expect("push frame");

        let completed = match vm.try_native(&func, 0) {
            NativeAttempt::Completed(VmValue::Int(99)) => true,
            NativeAttempt::Completed(value) => {
                panic!("parent completed with wrong value: {value:?}")
            }
            NativeAttempt::Resumed | NativeAttempt::Fallback => false,
        };
        if completed {
            let VmValue::Struct(updated) = vm.reg(0) else {
                panic!("expected updated Box in reg 0, got {:?}", vm.reg(0));
            };
            assert_eq!(updated.fields.first(), Some(&VmValue::Int(99)));
        }
        let stats = &vm.native.as_ref().expect("native").stats;
        assert_eq!(
            stats.native_call_edges, 0,
            "the rejected mut-handle edge must not be compiled, stats={stats:?}",
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
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::StringLiteral,
                    ..
                }
            )),
            "allocating StringLiteral helpers must not be memoized; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_region_does_not_memoize_invariant_json_parse_handle() {
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
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::JsonParse,
                    ..
                }
            )),
            "allocating JsonParse helpers must not be memoized; jit code: {:#?}",
            jit.code,
        );
        assert!(
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::JsonFieldInt,
                    ..
                }
            )),
            "JsonFieldInt cannot be invariant when its JsonParse input is recomputed; jit code: {:#?}",
            jit.code,
        );
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn native_osr_preserves_heap_live_through_scalar_loop() {
        let source = r#"fn main() -> Unit {
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
    fn native_region_does_not_memoize_handle_field_loads() {
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
            !jit.code.iter().any(|instr| matches!(
                instr,
                vm_jit::JitInstr::MemoizedHostCall {
                    helper: vm_jit::HostHelper::FieldHandle,
                    ..
                }
            )),
            "handle-returning FieldHandle helpers must not be memoized; jit code: {:#?}",
            jit.code,
        );
    }
