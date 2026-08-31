    use super::*;

    fn debug() -> Vec<MirFunctionDebug> {
        vec![MirFunctionDebug::new("main", vec!["value".into()])]
    }

    fn signature() -> MirFunctionSignature {
        MirFunctionSignature::new(Vec::new(), TypeId::new(0), false)
    }

    fn taking_signature() -> MirFunctionSignature {
        MirFunctionSignature::with_modes(
            vec![TypeId::new(0)],
            vec![MirParameterMode::Take],
            TypeId::new(0),
            false,
        )
    }

    #[test]
    fn builtin_registry_exposes_a_versioned_contract() {
        let id = builtin_id("String", "len").expect("String.len is catalog-owned");
        let descriptor = builtin_descriptor(id).expect("catalog identity has a descriptor");

        assert_eq!(BUILTIN_REGISTRY_SCHEMA, "rsscript.builtin_registry.v2");
        assert_eq!(BUILTIN_REGISTRY_DIGEST.len(), 64);
        assert_eq!(descriptor.id, id);
        assert_eq!(descriptor.namespace, "String");
        assert_eq!(descriptor.name, "len");
        assert_eq!(descriptor.vm_name, "StringLen");
        assert_eq!(
            descriptor.signature,
            "pub fn String.len(value: String) -> Int"
        );
        assert_eq!(
            descriptor.signature_source,
            BuiltinSignatureSource::Interface
        );
        assert_eq!(descriptor.determinism, BuiltinDeterminism::Deterministic);
        assert_eq!(descriptor.cost, BuiltinCost::InputDependent);
        assert_eq!(descriptor.class, BuiltinClass::DeterministicBuiltin);

        let primitive = builtin_descriptor(
            builtin_id("Channel", "bounded").expect("Channel.bounded is catalog-owned"),
        )
        .expect("stateful builtin has an explicit descriptor");
        assert_eq!(primitive.class, BuiltinClass::VmPrimitive);

        let internal = builtin_descriptor(
            builtin_id("Clone", "clone").expect("internal primitive is catalog-owned"),
        )
        .expect("internal primitive has an explicit descriptor");
        assert_eq!(internal.signature_source, BuiltinSignatureSource::Internal);
        assert!(
            internal
                .signature
                .starts_with("internal builtin Clone.clone via ")
        );
    }

    #[test]
    fn instruction_source_maps_must_reference_one_existing_instruction_once() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::LoadLiteral {
                    destination: ValueId::new(0),
                    value: MirLiteral::Unit,
                }],
                MirTerminator::Return(None),
            )],
        );
        let location = MirSourceLocation::new("main.rss", 1, 1, 4);
        let invalid = MirModule::new(
            vec![WireType::Unit],
            vec![function.clone()],
            vec![
                MirFunctionDebug::new("main", vec![]).with_instruction_sources(vec![
                    MirInstructionSource::new(BlockId::new(0), 1, location.clone()),
                ]),
            ],
            vec![],
        );
        assert!(matches!(
            invalid,
            Err(MirValidationError::InvalidInstructionSourceIndex { .. })
        ));

        let duplicate = MirModule::new(
            vec![WireType::Unit],
            vec![function],
            vec![
                MirFunctionDebug::new("main", vec![]).with_instruction_sources(vec![
                    MirInstructionSource::new(BlockId::new(0), 0, location.clone()),
                    MirInstructionSource::new(BlockId::new(0), 0, location),
                ]),
            ],
            vec![],
        );
        assert!(matches!(
            duplicate,
            Err(MirValidationError::DuplicateInstructionSource { .. })
        ));
    }

    #[test]
    fn resource_lifetimes_require_canonical_type_and_release_before_return() {
        let resource = WireType::Resource {
            name: "host.fs.File".into(),
        };
        let valid = MirModule::new(
            vec![WireType::Unit, resource.clone()],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Unit,
                        },
                        MirInstruction::AcquireResource {
                            place: PlaceId::new(0),
                            resource_type: ResourceTypeId::new(1),
                            source: ValueId::new(0),
                        },
                        MirInstruction::ReleaseResource {
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(valid.is_ok());

        let leaked = MirModule::new(
            vec![WireType::Unit, resource],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Unit,
                        },
                        MirInstruction::AcquireResource {
                            place: PlaceId::new(0),
                            resource_type: ResourceTypeId::new(1),
                            source: ValueId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(matches!(
            leaked,
            Err(MirValidationError::ResourceLeak { .. })
        ));
    }

    #[test]
    fn resource_cleanup_is_required_on_every_reachable_return_edge() {
        let types = vec![
            WireType::Unit,
            WireType::Bool,
            WireType::Resource {
                name: "host.fs.File".into(),
            },
        ];
        let entry = BasicBlock::new(
            BlockId::new(0),
            vec![
                MirInstruction::LoadLiteral {
                    destination: ValueId::new(0),
                    value: MirLiteral::Bool(true),
                },
                MirInstruction::LoadLiteral {
                    destination: ValueId::new(1),
                    value: MirLiteral::Unit,
                },
                MirInstruction::AcquireResource {
                    place: PlaceId::new(0),
                    resource_type: ResourceTypeId::new(2),
                    source: ValueId::new(1),
                },
            ],
            MirTerminator::Branch {
                condition: ValueId::new(0),
                then_target: BlockId::new(1),
                else_target: BlockId::new(2),
            },
        );
        let released = BasicBlock::new(
            BlockId::new(1),
            vec![MirInstruction::ReleaseResource {
                place: PlaceId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let also_released = BasicBlock::new(
            BlockId::new(2),
            vec![MirInstruction::ReleaseResource {
                place: PlaceId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let valid = MirModule::new(
            types.clone(),
            vec![MirFunction::new(
                FunctionId::new(0),
                signature(),
                1,
                2,
                vec![entry.clone(), released.clone(), also_released],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(valid.is_ok(), "every branch releases the resource");

        let missing_release = BasicBlock::new(BlockId::new(2), vec![], MirTerminator::Return(None));
        let leaked = MirModule::new(
            types,
            vec![MirFunction::new(
                FunctionId::new(0),
                signature(),
                1,
                2,
                vec![entry, released, missing_release],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(matches!(
            leaked,
            Err(MirValidationError::ResourceLeak { .. })
        ));
    }

    #[test]
    fn task_groups_must_close_on_every_reachable_return_edge() {
        let entry = BasicBlock::new(
            BlockId::new(0),
            vec![
                MirInstruction::LoadLiteral {
                    destination: ValueId::new(0),
                    value: MirLiteral::Bool(true),
                },
                MirInstruction::Spawn {
                    task: TaskId::new(0),
                    group: TaskGroupId::new(0),
                    target: FunctionId::new(1),
                    arguments: vec![],
                },
            ],
            MirTerminator::Branch {
                condition: ValueId::new(0),
                then_target: BlockId::new(1),
                else_target: BlockId::new(2),
            },
        );
        let joined = BasicBlock::new(
            BlockId::new(1),
            vec![MirInstruction::Join {
                group: TaskGroupId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let also_joined = BasicBlock::new(
            BlockId::new(2),
            vec![MirInstruction::Join {
                group: TaskGroupId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let worker = MirFunction::new(
            FunctionId::new(1),
            MirFunctionSignature::new(vec![], TypeId::new(0), true),
            0,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![],
                MirTerminator::Return(None),
            )],
        );
        let valid = MirModule::new(
            vec![WireType::Unit, WireType::Bool],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    1,
                    vec![entry.clone(), joined.clone(), also_joined],
                ),
                worker.clone(),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(valid.is_ok(), "every branch drains the task group");

        let missing_join = BasicBlock::new(BlockId::new(2), vec![], MirTerminator::Return(None));
        let leaked = MirModule::new(
            vec![WireType::Unit, WireType::Bool],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    1,
                    vec![entry, joined, missing_join],
                ),
                worker,
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(matches!(leaked, Err(MirValidationError::TaskLeak { .. })));
    }

    #[test]
    fn select_consumes_each_live_task_exactly_once() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let invalid = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
                    0,
                    2,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Select {
                                tasks: vec![TaskId::new(0), TaskId::new(0)],
                                winner: ValueId::new(0),
                                value: ValueId::new(1),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(1))),
                    )],
                ),
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::new(vec![], TypeId::new(0), true),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        }],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(matches!(
            invalid,
            Err(MirValidationError::TaskNotLive {
                task,
                ..
            }) if task == TaskId::new(0)
        ));
    }

    #[test]
    fn structured_tasks_must_be_closed_before_return() {
        let worker = MirFunction::new(
            FunctionId::new(1),
            MirFunctionSignature::new(vec![], TypeId::new(0), true),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![],
                MirTerminator::Return(None),
            )],
        );
        let valid = MirModule::new(
            vec![WireType::Unit],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Await {
                                destination: ValueId::new(0),
                                task: TaskId::new(0),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
                worker.clone(),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(valid.is_ok());

        let leaked = MirModule::new(
            vec![WireType::Unit],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    0,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::Spawn {
                            task: TaskId::new(0),
                            group: TaskGroupId::new(0),
                            target: FunctionId::new(1),
                            arguments: vec![],
                        }],
                        MirTerminator::Return(None),
                    )],
                ),
                worker,
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(matches!(leaked, Err(MirValidationError::TaskLeak { .. })));
    }

    #[test]
    fn accepts_a_typed_branching_function() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            1,
            3,
            vec![
                BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Bool(true),
                    }],
                    MirTerminator::Branch {
                        condition: ValueId::new(0),
                        then_target: BlockId::new(1),
                        else_target: BlockId::new(2),
                    },
                ),
                BasicBlock::new(
                    BlockId::new(1),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(1),
                        value: MirLiteral::Int(1),
                    }],
                    MirTerminator::Return(Some(ValueId::new(1))),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(2),
                        value: MirLiteral::Int(0),
                    }],
                    MirTerminator::Return(Some(ValueId::new(2))),
                ),
            ],
        );
        let module = MirModule::new(
            vec![WireType::Int {
                bits: 64,
                signed: true,
            }],
            vec![function],
            debug(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(module.functions().len(), 1);
        assert_eq!(
            module.ty(TypeId::new(0)),
            Some(&WireType::Int {
                bits: 64,
                signed: true
            })
        );
    }

    #[test]
    fn rejects_value_defined_on_only_one_predecessor_of_a_join() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            2,
            vec![
                BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Bool(true),
                    }],
                    MirTerminator::Branch {
                        condition: ValueId::new(0),
                        then_target: BlockId::new(1),
                        else_target: BlockId::new(2),
                    },
                ),
                BasicBlock::new(
                    BlockId::new(1),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(1),
                        value: MirLiteral::Int(1),
                    }],
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    Vec::new(),
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(3),
                    Vec::new(),
                    MirTerminator::Return(Some(ValueId::new(1))),
                ),
            ],
        );
        let error = MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new())
            .expect_err("join must reject a non-dominating value");
        assert!(matches!(
            error,
            MirValidationError::ValueDoesNotDominate { block, value, .. }
                if block == BlockId::new(3) && value == ValueId::new(1)
        ));
    }

    #[test]
    fn rejects_undefined_values_and_targets() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Branch {
                    condition: ValueId::new(0),
                    then_target: BlockId::new(1),
                    else_target: BlockId::new(0),
                },
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidBlockTarget { .. })
        ));
    }

    #[test]
    fn rejects_function_signatures_that_reference_unknown_types() {
        let function = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(Vec::new(), TypeId::new(1), false),
            0,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidType { .. })
        ));
    }

    #[test]
    fn rejects_calls_to_unknown_function_ids() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Function(FunctionId::new(1)),
                    arguments: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidFunctionTarget { .. })
        ));
    }

    #[test]
    fn rejects_direct_calls_with_the_wrong_arity() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            taking_signature(),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Function(FunctionId::new(0)),
                    arguments: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["input".into()]),
            MirFunctionDebug::new("caller", Vec::new()),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![callee, caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::CallArityMismatch {
                expected: 1,
                actual: 0,
                ..
            })
        ));
    }

    #[test]
    fn rejects_call_arguments_with_the_wrong_ownership_mode() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::with_modes(
                vec![TypeId::new(0)],
                vec![MirParameterMode::Mut],
                TypeId::new(0),
                false,
            ),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Function(FunctionId::new(0)),
                    arguments: vec![MirCallArgument::BorrowRead(PlaceId::new(0))],
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![callee, caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::CallArgumentModeMismatch {
                expected: MirParameterMode::Mut,
                actual: MirCallArgumentMode::Read,
                ..
            })
        ));
    }

    #[test]
    fn rejects_reading_a_place_after_it_is_taken() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            taking_signature(),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Call {
                        destination: ValueId::new(0),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![MirCallArgument::Take(PlaceId::new(0))],
                    },
                    MirInstruction::ReadPlace {
                        destination: ValueId::new(1),
                        place: PlaceId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![callee, caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::UseAfterMove { .. })
        ));
    }

    #[test]
    fn explicit_retain_keeps_a_place_live_but_drop_invalidates_it() {
        let retained = MirFunction::new(
            FunctionId::new(0),
            signature(),
            1,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Retain {
                        place: PlaceId::new(0),
                    },
                    MirInstruction::ReadPlace {
                        destination: ValueId::new(0),
                        place: PlaceId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(MirModule::new(vec![WireType::Unit], vec![retained], debug(), Vec::new()).is_ok());

        let dropped = MirFunction::new(
            FunctionId::new(0),
            signature(),
            1,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Drop {
                        place: PlaceId::new(0),
                    },
                    MirInstruction::ReadPlace {
                        destination: ValueId::new(0),
                        place: PlaceId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![dropped], debug(), Vec::new()),
            Err(MirValidationError::UseAfterMove { .. })
        ));
    }

    fn unit_taking_callee() -> MirFunction {
        MirFunction::new(
            FunctionId::new(0),
            taking_signature(),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        )
    }

    #[test]
    fn rejects_a_read_after_take_on_one_branch_at_a_join() {
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            3,
            vec![
                BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Bool(true),
                    }],
                    MirTerminator::Branch {
                        condition: ValueId::new(0),
                        then_target: BlockId::new(1),
                        else_target: BlockId::new(2),
                    },
                ),
                BasicBlock::new(
                    BlockId::new(1),
                    vec![MirInstruction::Call {
                        destination: ValueId::new(1),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![MirCallArgument::Take(PlaceId::new(0))],
                    }],
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    Vec::new(),
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(3),
                    vec![MirInstruction::ReadPlace {
                        destination: ValueId::new(2),
                        place: PlaceId::new(0),
                    }],
                    MirTerminator::Return(Some(ValueId::new(2))),
                ),
            ],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![unit_taking_callee(), caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::UseAfterMove { .. })
        ));
    }

    #[test]
    fn permits_reinitialization_after_a_branch_local_take() {
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            4,
            vec![
                BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Bool(true),
                    }],
                    MirTerminator::Branch {
                        condition: ValueId::new(0),
                        then_target: BlockId::new(1),
                        else_target: BlockId::new(2),
                    },
                ),
                BasicBlock::new(
                    BlockId::new(1),
                    vec![MirInstruction::Call {
                        destination: ValueId::new(1),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![MirCallArgument::Take(PlaceId::new(0))],
                    }],
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    Vec::new(),
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(3),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(2),
                            value: MirLiteral::Int(42),
                        },
                        MirInstruction::WritePlace {
                            place: PlaceId::new(0),
                            value: ValueId::new(2),
                        },
                        MirInstruction::ReadPlace {
                            destination: ValueId::new(3),
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(3))),
                ),
            ],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        MirModule::new(
            vec![WireType::Unit],
            vec![unit_taking_callee(), caller],
            debug,
            Vec::new(),
        )
        .expect("write reinitializes a place on every path after the join");
    }

    #[test]
    fn rejects_record_construction_without_a_named_layout_type() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Unit,
                    },
                    MirInstruction::MakeStruct {
                        destination: ValueId::new(1),
                        ty: TypeId::new(0),
                        fields: vec![("value".into(), ValueId::new(0))],
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidRecordType { .. })
        ));
    }

    #[test]
    fn rejects_invalid_builtin_type_metadata_and_runtime_layouts() {
        let decode = builtin_id("Json", "decode").expect("JSON decode is catalog-owned");
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Builtin {
                        id: decode,
                        parameter_modes: vec![MirParameterMode::Read].into_boxed_slice(),
                        type_arguments: vec![TypeId::new(0), TypeId::new(0)].into_boxed_slice(),
                    },
                    arguments: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::BuiltinTypeArgumentArity { .. })
        ));

        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        assert!(matches!(
            MirModule::with_type_layouts(
                vec![WireType::Named {
                    package: None,
                    name: "Actual".into(),
                    arguments: Vec::new(),
                }],
                vec![MirTypeLayout::new(TypeId::new(0), "Wrong", Vec::new())],
                vec![function],
                debug(),
                Vec::new(),
            ),
            Err(MirValidationError::InvalidTypeLayout { .. })
        ));
    }

    #[test]
    fn rejects_empty_or_signature_incompatible_dynamic_dispatch_tables() {
        let implementation = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(vec![TypeId::new(1)], TypeId::new(0), false),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = |parameter_modes: Box<[MirParameterMode]>,
                      dispatch: Box<[(TypeId, FunctionId)]>| {
            MirFunction::new(
                FunctionId::new(1),
                signature(),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::Call {
                        destination: ValueId::new(0),
                        target: MirCallTarget::Dynamic {
                            dispatch,
                            parameter_modes,
                        },
                        arguments: Vec::new(),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            )
        };
        let debug = || {
            vec![
                MirFunctionDebug::new("implementation", vec!["self".into()]),
                MirFunctionDebug::new("main", Vec::new()),
            ]
        };
        let types = || {
            vec![
                WireType::Unit,
                WireType::Named {
                    package: None,
                    name: "English".into(),
                    arguments: Vec::new(),
                },
            ]
        };

        assert!(matches!(
            MirModule::new(
                types(),
                vec![
                    implementation.clone(),
                    caller(
                        vec![MirParameterMode::Read].into_boxed_slice(),
                        Vec::new().into_boxed_slice()
                    ),
                ],
                debug(),
                Vec::new(),
            ),
            Err(MirValidationError::EmptyDynamicDispatch { .. })
        ));
        assert!(matches!(
            MirModule::new(
                types(),
                vec![
                    implementation,
                    caller(
                        vec![MirParameterMode::Take].into_boxed_slice(),
                        vec![(TypeId::new(1), FunctionId::new(0))].into_boxed_slice(),
                    ),
                ],
                debug(),
                Vec::new(),
            ),
            Err(MirValidationError::DynamicDispatchSignatureMismatch { .. })
        ));
    }

    #[test]
    fn closure_environment_contract_is_checked_before_codegen() {
        let closure = MirFunction::with_captures(
            FunctionId::new(0),
            MirFunctionSignature::new(Vec::new(), TypeId::new(0), false),
            vec![MirClosureCapture::new(
                TypeId::new(0),
                MirParameterMode::Read,
            )],
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::MakeClosure {
                    destination: ValueId::new(0),
                    function: FunctionId::new(0),
                    captures: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![closure, caller],
                vec![
                    MirFunctionDebug::new("closure", vec!["capture".into()]),
                    MirFunctionDebug::new("main", vec!["capture".into()]),
                ],
                Vec::new(),
            ),
            Err(MirValidationError::ClosureCaptureArityMismatch { .. })
        ));
    }
