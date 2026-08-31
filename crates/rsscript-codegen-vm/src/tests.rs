    use super::*;
    use rsscript_abi_model::{CORE_LIBRARY_ABI_VERSION, RUNTIME_ABI_VERSION, WireType};
    use rsscript_bytecode::BytecodeVerifier;
    use rsscript_mir::{
        BasicBlock, FunctionId, MirFunctionDebug, MirFunctionSignature, MirInstructionSource,
        MirSourceLocation, ResourceTypeId, TaskGroupId, TypeId,
    };

    #[test]
    fn scalar_cfg_emits_a_verifiable_vm_artifact_without_the_vm() {
        let module = MirModule::new(
            vec![WireType::Int {
                bits: 64,
                signed: true,
            }],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                3,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(20),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(1),
                            value: MirLiteral::Int(22),
                        },
                        MirInstruction::Binary {
                            destination: ValueId::new(2),
                            op: MirBinaryOp::Add,
                            left: ValueId::new(0),
                            right: ValueId::new(1),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(2))),
                )],
            )],
            vec![
                MirFunctionDebug::new("main", vec![]).with_instruction_sources(vec![
                    MirInstructionSource::new(
                        BlockId::new(0),
                        0,
                        MirSourceLocation::new("main.rss", 1, 1, 2),
                    ),
                ]),
            ],
            vec![],
        )
        .unwrap();
        let module = module.into_verified().expect("scalar MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .unwrap();
        assert_eq!(artifact.header.runtime_abi_version, RUNTIME_ABI_VERSION);
        assert_eq!(
            artifact.header.core_library_abi_version,
            CORE_LIBRARY_ABI_VERSION
        );
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode source-mapped payload");
        assert_eq!(
            payload["source_map"],
            serde_json::json!([{
                "function": 0,
                "instruction": 0,
                "file": "main.rss",
                "line": 1,
                "column": 1,
                "length": 2,
            }])
        );
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().unwrap())
            .unwrap();
        let facts = verified
            .typed_executable_facts()
            .expect("codegen attaches verified typed facts")
            .facts();
        assert_eq!(facts.executable_hash, artifact.header.executable_hash);
        assert_eq!(facts.functions.len(), 1);
        assert!(facts.functions[0].registers[..3].iter().all(|register| {
            matches!(
                register.ty,
                TypedFactTypeV1::Known(WireType::Int {
                    bits: 64,
                    signed: true
                })
            )
        }));
    }

    #[test]
    fn typed_facts_propagate_through_more_than_three_place_hops() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let mut instructions = vec![MirInstruction::LoadLiteral {
            destination: ValueId::new(0),
            value: MirLiteral::Int(7),
        }];
        for index in 0..5 {
            instructions.push(MirInstruction::WritePlace {
                place: PlaceId::new(index),
                value: ValueId::new(index),
            });
            instructions.push(MirInstruction::ReadPlace {
                destination: ValueId::new(index + 1),
                place: PlaceId::new(index),
            });
        }
        let module = MirModule::new(
            vec![int.clone()],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                5,
                6,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    instructions,
                    MirTerminator::Return(Some(ValueId::new(5))),
                )],
            )],
            vec![MirFunctionDebug::new(
                "main",
                (0..5).map(|index| format!("p{index}")).collect(),
            )],
            vec![],
        )
        .expect("place-chain MIR verifies")
        .into_verified()
        .expect("place-chain MIR admission");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit place-chain bytecode");
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect("verify place-chain facts");
        assert_eq!(
            verified
                .typed_executable_facts()
                .expect("typed facts")
                .facts()
                .functions[0]
                .registers[10]
                .ty,
            TypedFactTypeV1::Known(int)
        );
    }

    #[test]
    fn qualified_scalar_signature_survives_codegen_and_independent_verification() {
        let qualified_int = WireType::Qualified {
            qualifier: rsscript_abi_model::WireQualifier::Owned,
            value: Box::new(WireType::Int {
                bits: 64,
                signed: true,
            }),
        };
        let module = MirModule::new(
            vec![qualified_int.clone()],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::with_modes(
                    vec![TypeId::new(0)],
                    vec![MirParameterMode::Read],
                    TypeId::new(0),
                    false,
                ),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::ReadPlace {
                        destination: ValueId::new(0),
                        place: PlaceId::new(0),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            )],
            vec![MirFunctionDebug::new("identity", vec!["value".to_owned()])],
            vec![],
        )
        .expect("qualified scalar MIR verifies")
        .into_verified()
        .expect("qualified scalar MIR admission");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit qualified scalar bytecode");
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect("qualified scalar facts verify independently");
        let facts = verified
            .typed_executable_facts()
            .expect("qualified typed facts")
            .facts();
        assert_eq!(
            facts.functions[0].registers[0].ty,
            fact_type(Some(&qualified_int))
        );
        assert_eq!(
            facts.functions[0].registers[1].ty,
            fact_type(Some(&qualified_int))
        );
    }

    #[test]
    fn generic_direct_call_retains_bounded_type_arguments_without_changing_call_known() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let module = MirModule::new(
            vec![
                WireType::Named {
                    package: None,
                    name: "T".to_owned(),
                    arguments: Vec::new(),
                },
                int.clone(),
            ],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(1), false),
                    0,
                    2,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::LoadLiteral {
                                destination: ValueId::new(0),
                                value: MirLiteral::Int(7),
                            },
                            MirInstruction::Call {
                                destination: ValueId::new(1),
                                target: MirCallTarget::FunctionInstance {
                                    function: FunctionId::new(1),
                                    type_substitutions: vec![(TypeId::new(0), TypeId::new(1))]
                                        .into_boxed_slice(),
                                },
                                arguments: vec![MirCallArgument::Value(ValueId::new(0))],
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(1))),
                    )],
                ),
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::with_modes(
                        vec![TypeId::new(0)],
                        vec![MirParameterMode::Read],
                        TypeId::new(0),
                        false,
                    ),
                    1,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::ReadPlace {
                            destination: ValueId::new(0),
                            place: PlaceId::new(0),
                        }],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("identity", vec!["value".to_owned()]),
            ],
            vec![],
        )
        .expect("generic instance MIR verifies")
        .into_verified()
        .expect("generic instance MIR admission");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit generic call");
        let executable: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode executable");
        assert!(executable["functions"][0]["code"][1]["CallKnown"].is_object());
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect("v2 typed substitutions remain independently bounded");
        let facts = verified
            .typed_executable_facts()
            .expect("typed facts")
            .facts();
        assert_eq!(facts.schema, TYPED_EXECUTABLE_FACTS_SCHEMA_V2);
        assert_eq!(
            facts.functions[0].call_sites[0].type_parameters,
            vec![WireType::Named {
                package: None,
                name: "T".to_owned(),
                arguments: Vec::new(),
            }]
        );
        assert_eq!(facts.functions[0].call_sites[0].type_arguments, vec![int]);
    }

    #[test]
    fn aggregate_field_read_emits_verifiable_get_field_bytecode() {
        let module = MirModule::new(
            vec![WireType::Int {
                bits: 64,
                signed: true,
            }],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                3,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(42),
                        },
                        MirInstruction::MakeObject {
                            destination: ValueId::new(1),
                            fields: vec![("count".into(), ValueId::new(0))],
                        },
                        MirInstruction::GetField {
                            destination: ValueId::new(2),
                            base: ValueId::new(1),
                            field: "count".into(),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(2))),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec![])],
            vec![],
        )
        .expect("field MIR verifies");
        let module = module.into_verified().expect("field MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit field bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode field payload");
        assert_eq!(
            payload["functions"][0]["code"][2]["GetField"],
            serde_json::json!({"dst": 2, "base": 1, "name": "count"})
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode field bytecode"))
            .expect("verify field bytecode");
    }

    #[test]
    fn owned_list_construction_emits_a_verifiable_make_list_instruction() {
        let module = MirModule::new(
            vec![
                WireType::Int {
                    bits: 64,
                    signed: true,
                },
                WireType::List {
                    element: Box::new(WireType::Int {
                        bits: 64,
                        signed: true,
                    }),
                },
            ],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(1), false),
                0,
                3,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(1),
                            value: MirLiteral::Int(2),
                        },
                        MirInstruction::MakeList {
                            destination: ValueId::new(2),
                            items: vec![ValueId::new(0), ValueId::new(1)],
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(2))),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec![])],
            vec![],
        )
        .expect("list MIR verifies");
        let module = module.into_verified().expect("list MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit list bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode list payload");
        assert_eq!(
            payload["functions"][0]["code"][2]["MakeList"]["items"],
            serde_json::json!([0, 1])
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode list bytecode"))
            .expect("verify list bytecode");
    }

    #[test]
    fn map_lookup_emits_a_verifiable_option_valued_map_get_instruction() {
        let module = MirModule::new(
            vec![WireType::Unit],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                4,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(1),
                            value: MirLiteral::Int(42),
                        },
                        MirInstruction::MakeMap {
                            destination: ValueId::new(2),
                            entries: vec![(ValueId::new(0), ValueId::new(1))],
                        },
                        MirInstruction::MapGet {
                            destination: ValueId::new(3),
                            map: ValueId::new(2),
                            key: ValueId::new(0),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(3))),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec![])],
            vec![],
        )
        .expect("map-get MIR verifies");
        let module = module.into_verified().expect("map-get MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit map-get bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode map-get payload");
        assert_eq!(
            payload["functions"][0]["code"][3]["MapGet"],
            serde_json::json!({"dst": 3, "map": 2, "key": 0})
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode map-get bytecode"))
            .expect("verify map-get bytecode");
    }

    #[test]
    fn resource_lifetime_ops_preserve_resource_value_until_drop() {
        let module = MirModule::new(
            vec![
                WireType::Unit,
                WireType::Resource {
                    name: "host.test.Resource".into(),
                },
            ],
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
            vec![MirFunctionDebug::new("main", vec!["resource".into()])],
            vec![],
        )
        .unwrap();
        let module = module.into_verified().expect("resource MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit resource bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode resource payload");
        let opcodes = payload["functions"][0]["code"]
            .as_array()
            .expect("resource code")
            .iter()
            .map(|instruction| {
                instruction
                    .as_object()
                    .and_then(|instruction| instruction.keys().next())
                    .expect("single opcode")
                    .as_str()
            })
            .collect::<Vec<_>>();
        assert!(opcodes.contains(&"Move"));
        assert!(opcodes.contains(&"ResourceAcquire"));
        assert!(opcodes.contains(&"ResourceDrop"));
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode resource bytecode"))
            .expect("verify resource bytecode");
    }

    #[test]
    fn spawned_async_mir_emits_verifiable_task_bytecode() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let module = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
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
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::new(vec![], TypeId::new(0), true),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(7),
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
        )
        .unwrap();
        let module = module.into_verified().expect("task MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit task bytecode");
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode task bytecode"))
            .expect("verify task bytecode");
    }

    #[test]
    fn cancelled_child_mir_emits_verifiable_task_bytecode() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let module = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
                    0,
                    0,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Cancel {
                                task: TaskId::new(0),
                            },
                        ],
                        MirTerminator::Return(None),
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
                            value: MirLiteral::Int(7),
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
        )
        .expect("cancel MIR verifies");
        let module = module.into_verified().expect("cancel MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit cancellation bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode cancellation payload");
        assert!(
            payload["functions"][0]["code"]
                .as_array()
                .expect("cancellation code")
                .iter()
                .any(|instruction| instruction.get("CancelTask").is_some())
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode cancellation bytecode"))
            .expect("verify cancellation bytecode");
    }

    #[test]
    fn select_mir_emits_verifiable_first_ready_bytecode() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let worker = |id, value| {
            MirFunction::new(
                FunctionId::new(id),
                MirFunctionSignature::new(vec![], TypeId::new(0), true),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Int(value),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            )
        };
        let module = MirModule::new(
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
                            MirInstruction::Spawn {
                                task: TaskId::new(1),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(2),
                                arguments: vec![],
                            },
                            MirInstruction::Select {
                                tasks: vec![TaskId::new(0), TaskId::new(1)],
                                winner: ValueId::new(0),
                                value: ValueId::new(1),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(1))),
                    )],
                ),
                worker(1, 7),
                worker(2, 9),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("first", vec![]),
                MirFunctionDebug::new("second", vec![]),
            ],
            vec![],
        )
        .expect("select MIR verifies");
        let module = module.into_verified().expect("select MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit select bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode select payload");
        assert!(
            payload["functions"][0]["code"]
                .as_array()
                .expect("select code")
                .iter()
                .any(|instruction| instruction.get("SelectWait").is_some())
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode select bytecode"))
            .expect("verify select bytecode");
    }

    #[test]
    fn ownership_retain_and_drop_emit_a_verifiable_cleanup_boundary() {
        let module = MirModule::new(
            vec![WireType::Unit],
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
                        MirInstruction::WritePlace {
                            place: PlaceId::new(0),
                            value: ValueId::new(0),
                        },
                        MirInstruction::Retain {
                            place: PlaceId::new(0),
                        },
                        MirInstruction::Drop {
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["owned".into()])],
            vec![],
        )
        .expect("ownership MIR verifies");
        let module = module.into_verified().expect("ownership MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit ownership bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode ownership payload");
        let opcodes = payload["functions"][0]["code"]
            .as_array()
            .expect("ownership code")
            .iter()
            .map(|instruction| {
                instruction
                    .as_object()
                    .and_then(|instruction| instruction.keys().next())
                    .expect("single opcode")
                    .as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            opcodes,
            ["LoadUnit", "Move", "LoadUnit", "LoadUnit", "Return"],
            "retain has no VM side effect while drop clears its place before the unit return"
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode ownership bytecode"))
            .expect("verify ownership bytecode");
    }
