use super::*;
use crate::hir::{CallResolution, HirBlock, HirExpr, HirStmt};
use crate::syntax::ast::Callee;

fn span(line: usize) -> Span {
    Span {
        file: "test.rss".to_string(),
        line,
        column: 1,
        length: 1,
    }
}

fn successor_ids(step: &LocalFlowStep) -> Vec<usize> {
    step.successors.iter().map(|edge| edge.to).collect()
}

fn successor_drop_resources(step: &LocalFlowStep, to: usize) -> Vec<String> {
    step.successors
        .iter()
        .find(|edge| edge.to == to)
        .map_or_else(Vec::new, |edge| edge.drop_resources.clone())
}

#[test]
fn applies_move_and_retain_events_to_clean_local_state() {
    let mut state = BodyState::default();
    state.bind_local("image");
    state.bind_local("cached");

    state.apply_retention_events(&[HirEffectEvent {
        function_name: "run".to_string(),
        kind: HirEffectEventKind::Retain {
            callee: "Cache.store".to_string(),
            param: "value".to_string(),
        },
        binding_name: "cached".to_string(),
        span: span(10),
        value_span: span(10),
    }]);
    state.apply_move_events(&[HirEffectEvent {
        function_name: "run".to_string(),
        kind: HirEffectEventKind::Manage,
        binding_name: "image".to_string(),
        span: span(11),
        value_span: span(11),
    }]);

    assert!(!state.clean_locals.contains("cached"));
    assert!(!state.clean_locals.contains("image"));
    assert_eq!(state.moved["image"].line, 11);
}

#[test]
fn local_analysis_reports_fresh_return_issues_from_hir_flow_state() {
    let retain_event = HirEffectEvent {
        function_name: "render".to_string(),
        kind: HirEffectEventKind::Retain {
            callee: "RetainedImageStore.store".to_string(),
            param: "image".to_string(),
        },
        binding_name: "image".to_string(),
        span: span(2),
        value_span: span(2),
    };
    let body = HirFunctionBody {
        function_name: "render".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "image".to_string(),
                    value: Some(HirExpr::Call {
                        callee: Callee::Name("Image.load".to_string()),
                        receiver: None,
                        args: Vec::new(),
                        resolution: CallResolution::Unknown,
                        events: Vec::new(),
                        type_name: Some("Image".to_string()),
                        span: span(1),
                    }),
                    type_name: Some("Image".to_string()),
                    value_type_name: Some("Image".to_string()),
                    is_async: false,
                    span: span(1),
                },
                HirStmt::Expr(HirExpr::Call {
                    callee: Callee::Name("RetainedImageStore.store".to_string()),
                    receiver: None,
                    args: Vec::new(),
                    resolution: CallResolution::Unknown,
                    events: vec![retain_event],
                    type_name: None,
                    span: span(2),
                }),
                HirStmt::Return {
                    value: Some(HirExpr::Ident {
                        name: "image".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(3),
                    }),
                    proof: HirReturnProof::Ident {
                        name: "image".to_string(),
                    },
                    span: span(3),
                },
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };

    let local_analysis = LocalAnalysis::new(Some(&body));

    assert_eq!(
        local_analysis.fresh_return_issues(),
        vec![FreshReturnIssue {
            kind: FreshReturnIssueKind::NotClean {
                name: "image".to_string(),
            },
            span: span(3),
        }]
    );
}

#[test]
fn local_flow_steps_record_branch_successors() {
    let block = HirBlock {
        statements: vec![
            HirStmt::Let {
                kind: HirBindingKind::LocalLet,
                name: "seed".to_string(),
                value: None,
                type_name: Some("Image".to_string()),
                value_type_name: None,
                is_async: false,
                span: span(1),
            },
            HirStmt::If {
                condition: HirExpr::Ident {
                    name: "enabled".to_string(),
                    type_name: Some("Bool".to_string()),
                    span: span(2),
                },
                then_body: HirBlock {
                    statements: vec![HirStmt::Expr(HirExpr::Ident {
                        name: "left".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(3),
                    })],
                    span: span(3),
                },
                else_body: Some(HirBlock {
                    statements: vec![HirStmt::Expr(HirExpr::Ident {
                        name: "right".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(4),
                    })],
                    span: span(4),
                }),
                span: span(2),
            },
            HirStmt::Return {
                value: Some(HirExpr::Ident {
                    name: "done".to_string(),
                    type_name: Some("Image".to_string()),
                    span: span(5),
                }),
                proof: HirReturnProof::Ident {
                    name: "done".to_string(),
                },
                span: span(5),
            },
        ],
        span: span(1),
    };

    let steps = collect_local_flow_steps(&block);

    assert_eq!(steps.len(), 5);
    assert_eq!(steps[0].id, 0);
    assert_eq!(successor_ids(&steps[0]), vec![1]);
    assert_eq!(steps[1].kind, LocalFlowStepKind::Branch);
    assert_eq!(successor_ids(&steps[1]), vec![2, 3]);
    assert_eq!(successor_ids(&steps[2]), vec![4]);
    assert_eq!(successor_ids(&steps[3]), vec![4]);
    assert!(successor_ids(&steps[4]).is_empty());
}

#[test]
fn local_flow_steps_record_loop_break_and_continue_edges() {
    let block = HirBlock {
        statements: vec![
            HirStmt::Loop {
                condition: Some(HirExpr::Ident {
                    name: "keep_going".to_string(),
                    type_name: Some("Bool".to_string()),
                    span: span(1),
                }),
                body: HirBlock {
                    statements: vec![HirStmt::If {
                        condition: HirExpr::Ident {
                            name: "again".to_string(),
                            type_name: Some("Bool".to_string()),
                            span: span(2),
                        },
                        then_body: HirBlock {
                            statements: vec![HirStmt::Continue(span(3))],
                            span: span(3),
                        },
                        else_body: Some(HirBlock {
                            statements: vec![HirStmt::Break(span(4))],
                            span: span(4),
                        }),
                        span: span(2),
                    }],
                    span: span(2),
                },
                span: span(1),
            },
            HirStmt::Return {
                value: Some(HirExpr::Ident {
                    name: "done".to_string(),
                    type_name: Some("Image".to_string()),
                    span: span(5),
                }),
                proof: HirReturnProof::Ident {
                    name: "done".to_string(),
                },
                span: span(5),
            },
        ],
        span: span(1),
    };

    let steps = collect_local_flow_steps(&block);

    assert_eq!(steps.len(), 5);
    assert_eq!(steps[0].kind, LocalFlowStepKind::Loop);
    assert_eq!(successor_ids(&steps[0]), vec![1, 4]);
    assert_eq!(steps[1].kind, LocalFlowStepKind::Branch);
    assert_eq!(successor_ids(&steps[1]), vec![2, 3]);
    assert_eq!(steps[2].kind, LocalFlowStepKind::Continue);
    assert_eq!(successor_ids(&steps[2]), vec![0]);
    assert_eq!(steps[3].kind, LocalFlowStepKind::Break);
    assert_eq!(successor_ids(&steps[3]), vec![4]);
    assert!(successor_ids(&steps[4]).is_empty());
}

#[test]
fn local_flow_steps_do_not_route_unreachable_body_exits() {
    let block = HirBlock {
        statements: vec![
            HirStmt::Loop {
                condition: None,
                body: HirBlock {
                    statements: vec![
                        HirStmt::Break(span(2)),
                        HirStmt::Expr(HirExpr::Ident {
                            name: "unreachable".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(3),
                        }),
                    ],
                    span: span(2),
                },
                span: span(1),
            },
            HirStmt::Return {
                value: Some(HirExpr::Ident {
                    name: "done".to_string(),
                    type_name: Some("Image".to_string()),
                    span: span(4),
                }),
                proof: HirReturnProof::Ident {
                    name: "done".to_string(),
                },
                span: span(4),
            },
        ],
        span: span(1),
    };

    let steps = collect_local_flow_steps(&block);

    assert_eq!(steps.len(), 4);
    assert_eq!(steps[0].kind, LocalFlowStepKind::Loop);
    assert_eq!(successor_ids(&steps[0]), vec![1]);
    assert_eq!(steps[1].kind, LocalFlowStepKind::Break);
    assert_eq!(successor_ids(&steps[1]), vec![3]);
    assert!(successor_ids(&steps[2]).is_empty());
    assert!(successor_ids(&steps[3]).is_empty());
}

#[test]
fn local_flow_states_scope_with_resource_bindings() {
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::With {
                    resource: HirExpr::Call {
                        callee: Callee::Name("File.open".to_string()),
                        receiver: None,
                        args: Vec::new(),
                        resolution: CallResolution::Unknown,
                        events: Vec::new(),
                        type_name: Some("File".to_string()),
                        span: span(1),
                    },
                    binding: "file".to_string(),
                    body: HirBlock {
                        statements: vec![HirStmt::Expr(HirExpr::Ident {
                            name: "file".to_string(),
                            type_name: Some("File".to_string()),
                            span: span(2),
                        })],
                        span: span(2),
                    },
                    span: span(1),
                },
                HirStmt::Return {
                    value: Some(HirExpr::Ident {
                        name: "done".to_string(),
                        type_name: Some("Unit".to_string()),
                        span: span(3),
                    }),
                    proof: HirReturnProof::Ident {
                        name: "done".to_string(),
                    },
                    span: span(3),
                },
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));
    let steps = collect_local_flow_steps(body.block.as_ref().expect("block exists"));

    assert_eq!(successor_ids(&steps[0]), vec![1]);
    assert_eq!(successor_ids(&steps[1]), vec![2]);
    assert_eq!(
        successor_drop_resources(&steps[1], 2),
        vec!["file".to_string()]
    );
    assert!(
        local_analysis
            .flow_entry_state(&span(2))
            .is_some_and(|state| state.is_resource("file"))
    );
    assert_eq!(
        local_analysis
            .flow_entry_state(&span(2))
            .and_then(|state| state.value_type("file")),
        Some("File")
    );
    assert!(
        local_analysis
            .flow_entry_state(&span(3))
            .is_some_and(|state| !state.is_resource("file"))
    );
}

#[test]
fn local_analysis_indexes_resource_escape_facts_by_with_span() {
    let retain_event = HirEffectEvent {
        function_name: "run".to_string(),
        kind: HirEffectEventKind::Retain {
            callee: "register".to_string(),
            param: "file".to_string(),
        },
        binding_name: "file".to_string(),
        span: span(3),
        value_span: span(3),
    };
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![HirStmt::With {
                resource: HirExpr::Call {
                    callee: Callee::Name("File.open".to_string()),
                    receiver: None,
                    args: Vec::new(),
                    resolution: CallResolution::Unknown,
                    events: Vec::new(),
                    type_name: Some("File".to_string()),
                    span: span(1),
                },
                binding: "file".to_string(),
                body: HirBlock {
                    statements: vec![
                        HirStmt::Expr(HirExpr::Call {
                            callee: Callee::Name("register".to_string()),
                            receiver: None,
                            args: Vec::new(),
                            resolution: CallResolution::Unknown,
                            events: vec![retain_event],
                            type_name: None,
                            span: span(3),
                        }),
                        HirStmt::Let {
                            kind: HirBindingKind::ManagedLet,
                            name: "callback".to_string(),
                            value: Some(HirExpr::Closure {
                                params: Vec::new(),
                                captures: Vec::new(),
                                declared_effects: Vec::new(),
                                explicit: false,
                                body: HirBlock {
                                    statements: vec![HirStmt::Expr(HirExpr::Ident {
                                        name: "file".to_string(),
                                        type_name: Some("File".to_string()),
                                        span: span(5),
                                    })],
                                    span: span(4),
                                },
                                span: span(4),
                            }),
                            type_name: None,
                            value_type_name: None,
                            is_async: false,
                            span: span(4),
                        },
                    ],
                    span: span(2),
                },
                span: span(1),
            }],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));
    let escapes = local_analysis
        .resource_escapes(&span(1))
        .expect("with should have resource escape facts");

    assert_eq!(
        escapes,
        &[
            ResourceEscape {
                binding: "file".to_string(),
                kind: ResourceEscapeKind::Escape,
                span: span(3),
            },
            ResourceEscape {
                binding: "file".to_string(),
                kind: ResourceEscapeKind::Capture,
                span: span(4),
            },
        ]
    );
}

#[test]
fn local_flow_entry_states_merge_clean_local_retention() {
    let retain_event = HirEffectEvent {
        function_name: "run".to_string(),
        kind: HirEffectEventKind::Retain {
            callee: "Cache.store".to_string(),
            param: "value".to_string(),
        },
        binding_name: "image".to_string(),
        span: span(3),
        value_span: span(3),
    };
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "image".to_string(),
                    value: None,
                    type_name: Some("Image".to_string()),
                    value_type_name: None,
                    is_async: false,
                    span: span(1),
                },
                HirStmt::If {
                    condition: HirExpr::Ident {
                        name: "should_store".to_string(),
                        type_name: Some("Bool".to_string()),
                        span: span(2),
                    },
                    then_body: HirBlock {
                        statements: vec![HirStmt::Expr(HirExpr::Call {
                            callee: Callee::Name("Cache.store".to_string()),
                            receiver: None,
                            args: Vec::new(),
                            resolution: CallResolution::Unknown,
                            events: vec![retain_event],
                            type_name: None,
                            span: span(3),
                        })],
                        span: span(3),
                    },
                    else_body: None,
                    span: span(2),
                },
                HirStmt::Return {
                    value: Some(HirExpr::Ident {
                        name: "image".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(4),
                    }),
                    proof: HirReturnProof::Ident {
                        name: "image".to_string(),
                    },
                    span: span(4),
                },
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));

    let return_state = local_analysis
        .flow_entry_state(&span(4))
        .expect("return should be reachable");

    assert!(return_state.is_local("image"));
    assert!(!return_state.is_clean_local("image"));
}

#[test]
fn local_flow_entry_states_preserve_stable_value_types() {
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "config".to_string(),
                    value: None,
                    type_name: Some("InlineConfig".to_string()),
                    value_type_name: None,
                    is_async: false,
                    span: span(1),
                },
                HirStmt::If {
                    condition: HirExpr::Ident {
                        name: "enabled".to_string(),
                        type_name: Some("Bool".to_string()),
                        span: span(2),
                    },
                    then_body: HirBlock {
                        statements: vec![HirStmt::Expr(HirExpr::Ident {
                            name: "config".to_string(),
                            type_name: Some("InlineConfig".to_string()),
                            span: span(3),
                        })],
                        span: span(3),
                    },
                    else_body: None,
                    span: span(2),
                },
                HirStmt::Expr(HirExpr::Ident {
                    name: "config".to_string(),
                    type_name: Some("InlineConfig".to_string()),
                    span: span(4),
                }),
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));

    let expr_state = local_analysis
        .flow_entry_state(&span(4))
        .expect("expression should be reachable");

    assert_eq!(expr_state.value_type("config"), Some("InlineConfig"));
}

#[test]
fn local_analysis_indexes_managed_closure_uses_by_statement_span() {
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "image".to_string(),
                    value: None,
                    type_name: Some("Image".to_string()),
                    value_type_name: None,
                    is_async: false,
                    span: span(1),
                },
                HirStmt::Let {
                    kind: HirBindingKind::ManagedLet,
                    name: "callback".to_string(),
                    value: Some(HirExpr::Closure {
                        params: Vec::new(),
                        captures: Vec::new(),
                        declared_effects: Vec::new(),
                        explicit: false,
                        body: HirBlock {
                            statements: vec![HirStmt::Expr(HirExpr::Ident {
                                name: "image".to_string(),
                                type_name: Some("Image".to_string()),
                                span: span(3),
                            })],
                            span: span(2),
                        },
                        span: span(2),
                    }),
                    type_name: None,
                    value_type_name: None,
                    is_async: false,
                    span: span(2),
                },
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));

    assert_eq!(
        local_analysis.managed_closure_ident_uses(&span(2)),
        Some(&[("image".to_string(), span(3))][..])
    );
}

#[test]
fn local_flow_entry_states_carry_loop_break_moves() {
    let manage_event = HirEffectEvent {
        function_name: "run".to_string(),
        kind: HirEffectEventKind::Manage,
        binding_name: "image".to_string(),
        span: span(3),
        value_span: span(3),
    };
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "image".to_string(),
                    value: None,
                    type_name: Some("Image".to_string()),
                    value_type_name: None,
                    is_async: false,
                    span: span(1),
                },
                HirStmt::Loop {
                    condition: None,
                    body: HirBlock {
                        statements: vec![
                            HirStmt::Expr(HirExpr::Manage {
                                value: Box::new(HirExpr::Ident {
                                    name: "image".to_string(),
                                    type_name: Some("Image".to_string()),
                                    span: span(3),
                                }),
                                events: vec![manage_event],
                                type_name: Some("Image".to_string()),
                                span: span(3),
                            }),
                            HirStmt::Break(span(4)),
                        ],
                        span: span(3),
                    },
                    span: span(2),
                },
                HirStmt::Return {
                    value: Some(HirExpr::Ident {
                        name: "image".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(5),
                    }),
                    proof: HirReturnProof::Ident {
                        name: "image".to_string(),
                    },
                    span: span(5),
                },
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));

    let return_state = local_analysis
        .flow_entry_state(&span(5))
        .expect("return should be reachable");

    assert_eq!(
        return_state.move_span("image").map(|span| span.line),
        Some(3)
    );
    assert!(!return_state.is_clean_local("image"));
}

#[test]
fn local_analysis_reports_moved_uses_from_flow_state() {
    let manage_event = HirEffectEvent {
        function_name: "run".to_string(),
        kind: HirEffectEventKind::Manage,
        binding_name: "image".to_string(),
        span: span(2),
        value_span: span(2),
    };
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "image".to_string(),
                    value: None,
                    type_name: Some("Image".to_string()),
                    value_type_name: None,
                    is_async: false,
                    span: span(1),
                },
                HirStmt::Expr(HirExpr::Manage {
                    value: Box::new(HirExpr::Ident {
                        name: "image".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(2),
                    }),
                    events: vec![manage_event],
                    type_name: Some("Image".to_string()),
                    span: span(2),
                }),
                HirStmt::Expr(HirExpr::Ident {
                    name: "image".to_string(),
                    type_name: Some("Image".to_string()),
                    span: span(3),
                }),
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));

    assert_eq!(
        local_analysis.moved_uses(),
        vec![MovedUse {
            name: "image".to_string(),
            use_span: span(3),
            move_span: span(2),
        }]
    );
}

#[test]
fn local_analysis_reports_managed_to_local_uses_from_flow_state() {
    let body = HirFunctionBody {
        function_name: "run".to_string(),
        block: Some(HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::ManagedLet,
                    name: "image".to_string(),
                    value: None,
                    type_name: Some("Image".to_string()),
                    value_type_name: None,
                    is_async: false,
                    span: span(1),
                },
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "working".to_string(),
                    value: Some(HirExpr::Ident {
                        name: "image".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(2),
                    }),
                    type_name: Some("Image".to_string()),
                    value_type_name: Some("Image".to_string()),
                    is_async: false,
                    span: span(2),
                },
            ],
            span: span(1),
        }),
        ..HirFunctionBody::default()
    };
    let local_analysis = LocalAnalysis::new(Some(&body));

    assert_eq!(
        local_analysis.managed_to_local_uses(),
        vec![ManagedToLocalUse {
            local_name: "working".to_string(),
            managed_name: "image".to_string(),
            span: span(2),
        }]
    );
}
