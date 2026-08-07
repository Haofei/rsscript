#[cfg(test)]
mod register_window_tests {
    #[cfg(feature = "native-jit")]
    use crate::reg_vm::tier::native_scalar_callee_pending_on_branch_profile;

    use super::super::*;
    use crate::ExecutionFailureKind;

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

    fn is_allocation_failure(error: &EvalError) -> bool {
        matches!(
            error,
            EvalError::Execution {
                kind: ExecutionFailureKind::AllocationBudgetExceeded,
                ..
            }
        )
    }

    fn run_budgeted_instruction(
        code: Vec<RegInstr>,
        args: Vec<VmValue>,
    ) -> Result<VmValue, EvalError> {
        let mut function = RegFunction::placeholder("budgeted_instruction".to_string());
        function.params = args.len();
        function.regs = args.len() + 1;
        function.code = code;
        let function = Rc::new(function);
        let unit = Rc::new(RegUnit {
            functions: vec![Rc::clone(&function)],
            function_ids: [(function.name.clone(), 0)].into_iter().collect(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: true,
        });
        let mut vm = RegVm::new(Rc::clone(&unit), Vec::new(), HashMap::new());
        vm.prepare_frame(0, function.regs).expect("register window");
        for (index, value) in args.into_iter().enumerate() {
            vm.set_reg(index, value);
        }
        vm.set_limits(VmLimits {
            allocation_budget: Some(0),
            ..VmLimits::default()
        });
        vm.run_frame(&unit, function, 0)
    }

    #[test]
    fn failed_container_growth_does_not_mutate_shared_state() {
        let list = Rc::new(RefCell::new(TypedVec::Ints(Vec::new())));
        let error = run_budgeted_instruction(
            vec![
                RegInstr::ListPush {
                    dst: 2,
                    list: 0,
                    value: 1,
                },
                RegInstr::Return { src: 2 },
            ],
            vec![VmValue::List(Rc::clone(&list)), VmValue::Int(1)],
        )
        .expect_err("list growth must exceed a zero-byte budget");
        assert!(is_allocation_failure(&error));
        assert_eq!(list.borrow().len(), 0);
        assert_eq!(list.borrow().capacity(), 0);

        let deque = Rc::new(RefCell::new(VecDeque::new()));
        let error = run_budgeted_instruction(
            vec![
                RegInstr::DequePushBack {
                    dst: 2,
                    deque: 0,
                    value: 1,
                },
                RegInstr::Return { src: 2 },
            ],
            vec![VmValue::Deque(Rc::clone(&deque)), VmValue::Int(1)],
        )
        .expect_err("deque growth must exceed a zero-byte budget");
        assert!(is_allocation_failure(&error));
        assert!(deque.borrow().is_empty());
        assert_eq!(deque.borrow().capacity(), 0);

        let sorted_set = Rc::new(RefCell::new(TypedVec::Ints(Vec::new())));
        let error = run_budgeted_instruction(
            vec![
                RegInstr::SortedSetInsert {
                    dst: 2,
                    set: 0,
                    value: 1,
                },
                RegInstr::Return { src: 2 },
            ],
            vec![VmValue::List(Rc::clone(&sorted_set)), VmValue::Int(1)],
        )
        .expect_err("sorted-set growth must exceed a zero-byte budget");
        assert!(is_allocation_failure(&error));
        assert_eq!(sorted_set.borrow().len(), 0);
        assert_eq!(sorted_set.borrow().capacity(), 0);

        let sorted_map = Rc::new(RefCell::new(TypedVec::Boxed(Vec::new())));
        let error = run_budgeted_instruction(
            vec![
                RegInstr::SortedMapInsert {
                    dst: 3,
                    map: 0,
                    key: 1,
                    value: 2,
                },
                RegInstr::Return { src: 3 },
            ],
            vec![
                VmValue::List(Rc::clone(&sorted_map)),
                VmValue::Int(1),
                VmValue::Int(2),
            ],
        )
        .expect_err("sorted-map growth must exceed a zero-byte budget");
        assert!(is_allocation_failure(&error));
        assert_eq!(sorted_map.borrow().len(), 0);
        assert_eq!(sorted_map.borrow().capacity(), 0);

        let builder = Rc::new(RefCell::new(VmValue::string(String::new())));
        let error = run_budgeted_instruction(
            vec![
                RegInstr::StringBuilderPush {
                    dst: 2,
                    builder: 0,
                    value: 1,
                },
                RegInstr::Return { src: 2 },
            ],
            vec![
                VmValue::Managed(Rc::clone(&builder)),
                VmValue::string("value".to_string()),
            ],
        )
        .expect_err("string-builder growth must exceed a zero-byte budget");
        assert!(is_allocation_failure(&error));
        assert!(matches!(&*builder.borrow(), VmValue::String(value) if value.is_empty()));
    }

    #[test]
    fn aggregate_constructors_charge_outer_storage_before_publication() {
        let layout = intern_layout(Rc::from("Box"), vec![Rc::from("value")]);
        let cases = [
            vec![
                RegInstr::MakeStruct {
                    dst: 1,
                    layout: Rc::clone(&layout),
                    fields: vec![("value".to_string(), 0)],
                },
                RegInstr::Return { src: 1 },
            ],
            vec![
                RegInstr::MakeVariant {
                    dst: 1,
                    layout,
                    fields: vec![("value".to_string(), 0)],
                },
                RegInstr::Return { src: 1 },
            ],
            vec![
                RegInstr::MakeList {
                    dst: 1,
                    items: vec![0],
                },
                RegInstr::Return { src: 1 },
            ],
            vec![
                RegInstr::MakeObject {
                    dst: 1,
                    fields: vec![("value".to_string(), 0)],
                },
                RegInstr::Return { src: 1 },
            ],
        ];
        for code in cases {
            let error = run_budgeted_instruction(code, vec![VmValue::Int(1)])
                .expect_err("aggregate outer storage must exceed a zero-byte budget");
            assert!(
                is_allocation_failure(&error),
                "{error:?}"
            );
        }

        let error = run_budgeted_instruction(
            vec![
                RegInstr::MakeMap {
                    dst: 2,
                    entries: vec![(0, 1)],
                },
                RegInstr::Return { src: 2 },
            ],
            vec![VmValue::Int(1), VmValue::Int(2)],
        )
        .expect_err("map outer storage must exceed a zero-byte budget");
        assert!(is_allocation_failure(&error));
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

    #[test]
    fn native_binding_return_respects_allocation_budget() {
        let unit = Rc::new(RegUnit {
            functions: Vec::new(),
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: true,
        });
        let binding = ExternalFunction::new(|_| Ok(NativeValue::String("x".repeat(1024))));
        let mut vm = RegVm::new(
            unit,
            Vec::new(),
            [("test.big".to_string(), binding)].into_iter().collect(),
        );
        vm.set_limits(VmLimits {
            allocation_budget: Some(64),
            ..VmLimits::default()
        });
        let error = vm
            .call_external_symbol("test.big", &[], &[], 0)
            .expect_err("large native result must be rejected before materialization");
        assert!(is_allocation_failure(&error));
    }

    #[test]
    fn native_binding_spare_capacity_respects_allocation_budget() {
        let unit = Rc::new(RegUnit {
            functions: Vec::new(),
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: true,
        });
        let string_binding = ExternalFunction::new(|_| {
            let mut value = String::with_capacity(1 << 20);
            value.push('x');
            Ok(NativeValue::String(value))
        });
        let json_binding = ExternalFunction::new(|_| {
            let values = Vec::with_capacity(1 << 16);
            Ok(NativeValue::Json(serde_json::Value::Array(values)))
        });
        let mut vm = RegVm::new(
            Rc::clone(&unit),
            Vec::new(),
            [
                ("test.string-capacity".to_string(), string_binding),
                ("test.json-capacity".to_string(), json_binding),
            ]
            .into_iter()
            .collect(),
        );
        vm.set_limits(VmLimits {
            allocation_budget: Some(64),
            ..VmLimits::default()
        });
        for key in ["test.string-capacity", "test.json-capacity"] {
            let error = vm
                .call_external_symbol(key, &[], &[], 0)
                .expect_err("spare native capacity must be charged before publication");
            assert!(
                is_allocation_failure(&error),
                "{key}: {error:?}"
            );
        }
    }

    #[test]
    fn native_binding_budget_failure_keeps_mutations_atomic() {
        let unit = Rc::new(RegUnit {
            functions: Vec::new(),
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: true,
        });
        let binding = ExternalFunction::new(|_| {
            Ok(NativeValue::List(vec![
                NativeValue::Unit,
                NativeValue::String("x".repeat(1024)),
            ]))
        });
        let mut vm = RegVm::new(
            unit,
            Vec::new(),
            [("test.mutate".to_string(), binding)].into_iter().collect(),
        );
        vm.set_limits(VmLimits {
            allocation_budget: Some(64),
            ..VmLimits::default()
        });
        vm.prepare_frame(0, 1).expect("register window");
        vm.set_reg(0, VmValue::Int(7));
        let error = vm
            .call_external_symbol("test.mutate", &[0], &[0], 0)
            .expect_err("mutation envelope must be rejected atomically");
        assert!(is_allocation_failure(&error));
        assert_eq!(vm.reg(0), &VmValue::Int(7));
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
    fn assert_eval_observables_eq(actual: &EvalOutput, expected: &EvalOutput) {
        assert_eq!(actual.value, expected.value);
        assert_eq!(actual.display_value, expected.display_value);
        assert_eq!(actual.native_value, expected.native_value);
        assert_eq!(actual.stdout, expected.stdout);
        assert_eq!(actual.stderr, expected.stderr);
        assert_eq!(actual.provider_call_traces, expected.provider_call_traces);
    }

    include!("register_window/lowering.rs");
    include!("register_window/translation.rs");
    include!("register_window/tiering_and_memo.rs");
    include!("register_window/abi_and_heap.rs");
    include!("register_window/osr_collections.rs");
    include!("register_window/closures.rs");
    include!("register_window/deopt_and_transactions.rs");
}
