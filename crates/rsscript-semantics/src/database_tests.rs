    use super::*;
    use crate::hir::{HirExpr, HirStmt};
    use crate::validate_sources_with_interfaces;
    use rsscript_operation::{CancellationToken, MonotonicDeadline};
    use std::time::{Duration, Instant};

    #[test]
    fn source_snapshot_owns_the_captured_text() {
        let mut source = "fn main() -> Unit { return Unit }\n".to_string();
        let snapshot = SourceSnapshot::single("main.rss", &source);
        source.clear();

        assert_eq!(
            snapshot.files()[0].text(),
            "fn main() -> Unit { return Unit }\n"
        );
    }

    #[test]
    fn validated_program_requires_complete_error_free_analysis() {
        let validated = crate::validate_source("main.rss", "fn main() -> Unit { return Unit }\n")
            .expect("clean source should validate");
        assert_eq!(validated.database().sources().files()[0].path(), "main.rss");
        assert_eq!(validated.database().source_programs().len(), 1);

        let diagnostics =
            crate::validate_source("invalid.rss", "fn main() -> Int { return Missing.value }\n")
                .expect_err("frontend errors must not construct ValidatedProgram");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
    }

    #[test]
    fn semantic_database_interns_shared_signature_and_field_types() {
        let source = r#"
struct Holder<U> {
    value: U
}

fn first<U, W>(left: read U, right: read W) -> U {
    return left
}

fn main() -> Unit {
    let value: Int = first(left: read 1, right: read "unused")
    return Unit
}
"#;
        let validated =
            crate::validate_source("structural-types.rss", source).expect("valid source");
        let database = validated.database();
        let types = database.hir().semantic_types();
        let first = types
            .functions()
            .find(|(name, _)| *name == "first")
            .map(|(_, facts)| facts)
            .expect("first signature facts");
        let holder = types.named_type("Holder").expect("Holder type facts");

        assert_eq!(first.type_parameters.as_ref(), ["U", "W"]);
        assert_eq!(types.arena().get(first.parameters[0].1).to_string(), "U");
        assert_eq!(types.arena().get(first.parameters[1].1).to_string(), "W");
        assert_eq!(
            first
                .return_type
                .map(|ty| types.arena().get(ty).to_string())
                .as_deref(),
            Some("U")
        );
        assert_eq!(
            first.parameters[0].1, holder.fields[0].1,
            "structurally identical U facts must share one TypeId"
        );
        assert!(database.interned_type_count() >= 3);
    }

    #[test]
    fn frontend_input_snapshot_keeps_source_and_interface_roles_separate() {
        let input = FrontendInputSnapshot::from_sources(
            [("main.rss", "fn main() -> Unit { return Unit }")],
            [("host.rssi", "module host\npub fn value() -> Int")],
        );
        assert_eq!(input.sources().files()[0].path(), "main.rss");
        assert_eq!(input.interfaces().files()[0].path(), "host.rssi");
        assert_eq!(input.sources().files()[0].file_id(), FileId::new(0));
        assert_eq!(input.interfaces().files()[0].file_id(), FileId::new(0));
    }

    #[test]
    fn checked_hir_retains_declared_closure_contracts() {
        let validated = validate_sources_with_interfaces(
            &[(
                "closure-contract.rss",
                r#"
fn main() -> Int {
    let offset = 40
    let add: Fn(Int) -> Int = fn(value) captures(read offset) {
        return value + offset
    }
    return add(2)
}
"#,
            )],
            &[],
        )
        .expect("annotated closure source validates");
        let body = validated
            .database()
            .hir()
            .function_body("main")
            .expect("main HIR body exists");
        let block = body.block.as_ref().expect("main body is lowered");
        let HirStmt::Let {
            value: Some(HirExpr::Closure { ty: Some(ty), .. }),
            ..
        } = &block.statements[1]
        else {
            panic!("closure must retain its structural Fn contract")
        };

        assert!(ty.is_function());
        assert_eq!(ty.to_string(), "Fn(read Int) -> Int");
    }

    #[test]
    fn snapshot_assigns_repeatable_file_identity_and_initial_revision() {
        let snapshot = SourceSnapshot::from_sources([("a.rss", "a"), ("b.rss", "b")]);
        let first = &snapshot.files()[0];
        let second = &snapshot.files()[1];
        assert_eq!(first.file_id(), FileId::new(0));
        assert_eq!(second.file_id(), FileId::new(1));
        assert_eq!(first.revision(), SourceRevision::INITIAL);
        assert_eq!(snapshot.file(FileId::new(1)).unwrap().path(), "b.rss");
        assert!(snapshot.file(FileId::new(2)).is_none());
    }

    #[test]
    fn compilation_session_tracks_replacements_removals_and_deterministic_snapshots() {
        let mut session = CompilationSession::default();
        let beta = session.set_file("b.rss", "one").unwrap();
        let alpha = session.set_file("a.rss", "two").unwrap();
        assert_eq!(beta.file_id, FileId::new(0));
        assert_eq!(alpha.file_id, FileId::new(1));

        let first = session.source_snapshot();
        assert_eq!(
            first
                .files()
                .iter()
                .map(SourceFileSnapshot::path)
                .collect::<Vec<_>>(),
            ["a.rss", "b.rss"]
        );
        assert_eq!(first.file(beta.file_id).unwrap().text(), "one");

        let unchanged = session.set_file("b.rss", "one").unwrap();
        assert_eq!(
            unchanged,
            SourceUpdate {
                changed: false,
                ..beta
            }
        );
        let replacement = session.set_file("b.rss", "three").unwrap();
        assert_eq!(replacement.file_id, beta.file_id);
        assert_eq!(replacement.revision, SourceRevision::new(1));
        assert!(replacement.changed);

        assert_eq!(session.remove_file("a.rss").unwrap(), alpha);
        assert!(session.remove_file("a.rss").is_none());
        assert_eq!(session.source_snapshot().files().len(), 1);
        let interface = session.set_interface("host.rssi", "module host").unwrap();
        assert_eq!(interface.file_id, FileId::new(0));
        assert_eq!(session.interface_snapshot().files()[0].path(), "host.rssi");
    }

    #[test]
    fn compilation_session_caches_parse_queries_by_immutable_revision() {
        let mut session = CompilationSession::default();
        let source = session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        let first = session.parse_file("main.rss").unwrap();
        let second = session.parse_file("main.rss").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            session.stats(),
            CompilationSessionStats {
                parse_cache_hits: 1,
                parse_cache_misses: 1,
                hir_cache_hits: 0,
                hir_cache_misses: 0,
                workspace_hir_cache_hits: 0,
                workspace_hir_cache_misses: 0,
                workspace_type_cache_hits: 0,
                workspace_type_cache_misses: 0,
                workspace_analysis_cache_hits: 0,
                workspace_analysis_cache_misses: 0,
                module_header_cache_hits: 0,
                module_header_cache_misses: 0,
                workspace_module_graph_cache_hits: 0,
                workspace_module_graph_cache_misses: 0,
                workspace_diagnostic_cache_hits: 0,
                workspace_diagnostic_cache_misses: 0,
                ..CompilationSessionStats::default()
            }
        );

        let unchanged = session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        assert_eq!(unchanged.file_id, source.file_id);
        assert!(!unchanged.changed);
        assert!(Arc::ptr_eq(
            &first,
            &session.parse_file("main.rss").unwrap()
        ));

        session
            .set_file("main.rss", "fn main() -> Unit { let x = Unit return x }")
            .unwrap();
        let replacement = session.parse_file("main.rss").unwrap();
        assert!(!Arc::ptr_eq(&first, &replacement));
        session.remove_file("main.rss");
        assert!(session.parse_file("main.rss").is_none());
    }

    #[test]
    fn compilation_session_owns_revisioned_editor_queries() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main()->Int{return 1}\n")
            .expect("source enters session");

        let first_format = session.format_file("main.rss").expect("format");
        let second_format = session.format_file("main.rss").expect("cached format");
        assert!(Arc::ptr_eq(&first_format, &second_format));
        let first_lint = session.lint_file("main.rss").expect("lint");
        let second_lint = session.lint_file("main.rss").expect("cached lint");
        assert!(Arc::ptr_eq(&first_lint, &second_lint));
        let first_symbols = session.symbol_index_file("main.rss").expect("symbols");
        let second_symbols = session
            .symbol_index_file("main.rss")
            .expect("cached symbols");
        assert!(Arc::ptr_eq(&first_symbols, &second_symbols));
        let first_document_symbols = session
            .document_symbols_file("main.rss")
            .expect("document symbols");
        let second_document_symbols = session
            .document_symbols_file("main.rss")
            .expect("cached document symbols");
        assert!(Arc::ptr_eq(
            &first_document_symbols,
            &second_document_symbols
        ));

        let stats = session.stats();
        assert_eq!((stats.format_cache_misses, stats.format_cache_hits), (1, 1));
        assert_eq!((stats.lint_cache_misses, stats.lint_cache_hits), (1, 1));
        assert_eq!((stats.symbol_cache_misses, stats.symbol_cache_hits), (1, 1));
        assert_eq!(
            (
                stats.document_symbol_cache_misses,
                stats.document_symbol_cache_hits,
            ),
            (1, 1)
        );

        session
            .set_file("main.rss", "fn main() -> Int { return 2 }\n")
            .expect("replacement invalidates editor queries");
        let formatted = session.format_file("main.rss").expect("replacement format");
        assert!(formatted.contains("return 2"));
        let symbols = session
            .document_symbols_file("main.rss")
            .expect("replacement document symbols");
        assert_eq!(symbols[0].name, "main");
        let stats = session.stats();
        assert_eq!(stats.format_cache_misses, 2);
        assert_eq!(stats.document_symbol_cache_misses, 2);
    }

    #[test]
    fn editor_queries_reject_cancelled_and_expired_cached_requests() {
        use std::time::{Duration, Instant};

        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }\n")
            .expect("source enters session");
        session
            .set_interface("host.rssi", "module host\npub fn emit() -> Unit\n")
            .expect("interface enters session");

        // Warm every cache that is exposed to editor clients. Operation-aware
        // queries must reject a later cancelled/expired request rather than
        // letting these cached values escape.
        session.format_file("main.rss");
        session.format_interface("host.rssi");
        session.lint_file("main.rss");
        session.lint_interface("host.rssi");
        session.symbol_index_file("main.rss");
        session.symbol_index_interface("host.rssi");
        session.document_symbols_file("main.rss");
        session.document_symbols_interface("host.rssi");
        session.syntax_diagnostics_interface("host.rssi");

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert_eq!(
            session.format_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.format_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.lint_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.lint_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert!(matches!(
            session.symbol_index_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert!(matches!(
            session.symbol_index_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert!(matches!(
            session.document_symbols_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert!(matches!(
            session.document_symbols_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert_eq!(
            session.syntax_diagnostics_interface_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert_eq!(
            session.format_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );
        assert!(matches!(
            session.symbol_index_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        ));
        assert!(matches!(
            session.document_symbols_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        ));
    }

    #[test]
    fn compilation_session_caches_syntax_diagnostics_by_revision() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();

        let first = session
            .syntax_diagnostics_file("main.rss")
            .expect("source exists");
        let second = session
            .syntax_diagnostics_file("main.rss")
            .expect("cached source exists");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(first.is_empty());

        session
            .set_file("main.rss", "fn main( { return Unit }")
            .unwrap();
        let replacement = session
            .syntax_diagnostics_file("main.rss")
            .expect("replacement source exists");
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert!(
            replacement
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            session.syntax_diagnostics_file_with_operation(
                "main.rss",
                &OperationContext {
                    cancellation: Some(cancellation),
                    ..OperationContext::default()
                },
            ),
            Err(OperationAbort::Cancelled)
        );
    }

    #[test]
    fn compilation_session_caches_workspace_diagnostics_for_one_input_snapshot() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn emit() -> Unit")
            .unwrap();
        let operation = OperationContext::default();
        let first = session
            .semantic_workspace_diagnostics_with_operation(&operation)
            .unwrap();
        let second = session
            .semantic_workspace_diagnostics_with_operation(&operation)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        session
            .set_interface("host.rssi", "module host\npub fn replacement() -> Unit")
            .unwrap();
        session
            .semantic_workspace_diagnostics_with_operation(&operation)
            .unwrap();
        assert_eq!(
            session.stats().workspace_diagnostic_cache_hits,
            1,
            "the unchanged query must be served from the session cache"
        );
        assert_eq!(session.stats().workspace_diagnostic_cache_misses, 2);
    }

    #[test]
    fn document_semantic_diagnostics_track_only_visible_interface_revisions() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.*\nfn main() -> Int { return value() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> Int\n")
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(
            first
                .iter()
                .all(|diagnostic| !diagnostic.severity.is_error())
        );
        let second = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &second));

        // The source imports `host`, not `other`, so this edit must retain the
        // same document query result while broad workspace diagnostics remain
        // free to recompute independently.
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> String\n")
            .unwrap();
        let unrelated = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &unrelated));
        assert_eq!(session.stats().semantic_document_diagnostic_cache_misses, 1);
        assert_eq!(session.stats().semantic_document_diagnostic_cache_hits, 2);

        // An imported contract changes the closure key and must force a fresh
        // check; the resulting return type mismatch proves the new contract is
        // the one observed by the document query.
        session
            .set_interface("host.rssi", "module host\npub fn value() -> String\n")
            .unwrap();
        let changed = session
            .semantic_diagnostics_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(
            changed
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
        assert_eq!(session.stats().semantic_document_diagnostic_cache_misses, 2);
    }

    #[test]
    fn document_semantic_analysis_reuses_resolve_type_and_hir_by_interface_closure() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.*\nfn main() -> Int { return value() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> Int\n")
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(first.database().hir().function_body("main").is_some());
        assert!(
            first
                .database()
                .hir()
                .semantic_types()
                .functions()
                .any(|(name, _)| name == "main")
        );
        let second = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &second));

        // The selected contract closure excludes `other`, so its edit cannot
        // invalidate already-resolved type/HIR facts for `main`.
        session
            .set_interface("other.rssi", "module other\npub fn ignored() -> String\n")
            .unwrap();
        let unrelated = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &unrelated));

        // A visible interface revision must invalidate the complete semantic
        // result, not merely document diagnostics.
        session
            .set_interface("host.rssi", "module host\npub fn value() -> String\n")
            .unwrap();
        let changed = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(
            changed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
        assert_eq!(session.stats().semantic_document_analysis_cache_misses, 2);
        assert_eq!(session.stats().semantic_document_analysis_cache_hits, 2);
    }

    #[test]
    fn document_semantic_analysis_tracks_imported_source_revisions() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse lib.value\nfn main() -> Int { return value() }\n",
            )
            .unwrap();
        session
            .set_file("lib.rss", "module lib\nfn value() -> Int { return 1 }\n")
            .unwrap();
        session
            .set_file(
                "other.rss",
                "module other\nfn ignored() -> Int { return 1 }\n",
            )
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(first.diagnostics().is_empty());
        assert!(first.database().hir().function_body("lib__value").is_some());

        // Unrelated source edits retain the imported-source closure and its
        // cached resolve/type/HIR facts.
        session
            .set_file(
                "other.rss",
                "module other\nfn ignored() -> Int { return 2 }\n",
            )
            .unwrap();
        let unrelated = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(Arc::ptr_eq(&first, &unrelated));

        // A source module selected through `use` must invalidate the consumer
        // and produce diagnostics from the updated cross-source contract.
        session
            .set_file(
                "lib.rss",
                "module lib\nfn value() -> String { return \"changed\" }\n",
            )
            .unwrap();
        let changed = session
            .semantic_analysis_file_with_operation("main.rss", &operation)
            .unwrap()
            .expect("source exists");
        assert!(!Arc::ptr_eq(&first, &changed));
        assert!(
            changed
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity.is_error())
        );
    }

    #[test]
    fn compilation_session_caches_complete_workspace_analysis_and_validation() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Int { return Host.value() }")
            .unwrap();
        session
            .set_interface("host.rssi", "module Host\npub fn value() -> Int\n")
            .unwrap();
        let operation = OperationContext::default();

        let first = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        let second = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(
            session
                .workspace_validated_with_operation(&operation)
                .unwrap()
                .is_ok()
        );
        assert_eq!(session.stats().workspace_analysis_cache_misses, 1);
        assert_eq!(session.stats().workspace_analysis_cache_hits, 2);

        session
            .set_file("main.rss", "fn main() -> Int { return Host.next() }")
            .unwrap();
        let replacement = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().workspace_analysis_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_analysis_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
    }

    #[test]
    fn session_without_core_matches_the_explicit_no_core_entrypoint() {
        let source = "fn main() -> Int { return Host.value() }\n";
        let interfaces = [("host.rssi", "module Host\npub fn value() -> Int\n")];
        let expected = crate::analyze_sources_with_interfaces_without_core_result(
            &[("main.rss", source)],
            &interfaces,
        );

        let mut session = CompilationSession::without_core();
        assert_eq!(
            session.interface_policy(),
            SessionInterfacePolicy::WithoutCore
        );
        session.set_file("main.rss", source).unwrap();
        for (path, text) in interfaces {
            session.set_interface(path, text).unwrap();
        }
        let actual = session.workspace_analysis();
        assert_eq!(actual.diagnostics(), expected.diagnostics());

        let operation = OperationContext::default();
        let cached = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        assert!(Arc::ptr_eq(&actual, &cached));
    }

    #[test]
    fn session_standard_packages_match_the_legacy_standard_prelude() {
        // This source relies on generic and callback facts carried by the
        // standard package prelude. Injecting the public Core interfaces by
        // hand used to select a subtly different analysis flavor here.
        let source = r#"
struct Adder derives(Clone) {
    fxn: owned Fn(Int) -> Int
}

fn run() -> Int {
    local adders = List.new<Adder>()
    let base = 5
    let a = Adder(fxn: fn(x) captures(read base) { return x * base })
    List.push(list: mut adders, value: read a)
    let result = List.get(list: read adders, index: 0)
    return result.fxn(3)
}
"#;
        let expected = crate::analyze_source_result("standard-prelude.rss", source);
        let mut session = CompilationSession::with_standard_packages();
        assert_eq!(
            session.interface_policy(),
            SessionInterfacePolicy::WithStandardPackages
        );
        session.set_file("standard-prelude.rss", source).unwrap();

        let first = session.workspace_analysis();
        assert_eq!(first.diagnostics(), expected.diagnostics());

        let operation = OperationContext::default();
        let cached = session
            .workspace_analysis_with_operation(&operation)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &cached));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_analysis_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
    }

    #[test]
    fn standard_package_session_rejects_explicit_interfaces() {
        let mut session = CompilationSession::with_standard_packages();
        assert!(matches!(
            session.set_interface("host.rssi", "module Host\npub fn value() -> Int\n"),
            Err(SourceStoreError::InterfacesForbiddenByPolicy {
                policy: SessionInterfacePolicy::WithStandardPackages,
            })
        ));
        assert!(session.interface_snapshot().is_empty());
    }

    #[test]
    fn compilation_session_owns_and_invalidates_the_workspace_module_graph() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.api\nfn main() -> Unit {}\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host.api\npub fn emit() -> Unit\n")
            .unwrap();
        session
            .set_interface(
                "host-base.rssi",
                "module host.base\npub fn base() -> Unit\n",
            )
            .unwrap();
        session
            .set_interface("fallback.rssi", "pub fn fallback() -> Unit\n")
            .unwrap();

        let first = session.workspace_module_graph();
        assert_eq!(first.source("main.rss").unwrap().imports(), ["host.api"]);
        assert_eq!(
            first.interface("host.rssi").unwrap().modules(),
            ["host.api"]
        );
        assert_eq!(
            first.interface("fallback.rssi").unwrap().modules(),
            ["fallback"]
        );
        assert_eq!(
            first.visible_interface_paths("main.rss", ["host.api".to_string()]),
            BTreeSet::from(["host.rssi".to_string()])
        );
        let second = session.workspace_module_graph();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);
        assert_eq!(session.stats().workspace_module_graph_cache_hits, 1);

        session
            .set_interface(
                "host.rssi",
                "module host.api\nuse host.base\npub fn emit() -> Unit\n",
            )
            .unwrap();
        let replacement = session.workspace_module_graph();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(
            replacement.interface("host.rssi").unwrap().imports(),
            ["host.base"]
        );
        assert_eq!(
            replacement.interface_dependent_paths(
                &BTreeSet::from(["host.base".to_string()]),
                "host-base.rssi",
            ),
            BTreeSet::from(["host.rssi".to_string(), "main.rss".to_string()])
        );
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            session.workspace_module_graph_with_operation(&OperationContext {
                cancellation: Some(cancellation),
                ..OperationContext::default()
            }),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(session.stats().workspace_module_graph_cache_hits, 1);
    }

    #[test]
    fn workspace_module_graph_survives_implementation_only_edits() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nuse host.api\nfn main() -> Int { return Host.value() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host.api\npub fn value() -> Int\n")
            .unwrap();

        let first = session.workspace_module_graph();
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);

        // Changing executable bodies invalidates semantic queries, but does not
        // alter this syntax-only graph's node identity or import closure.
        session
            .set_file(
                "main.rss",
                "module app\nuse host.api\nfn main() -> Int { return Host.value() + 1 }\n",
            )
            .unwrap();
        let body_edit = session.workspace_module_graph();
        assert!(Arc::ptr_eq(&first, &body_edit));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);

        // An interface signature edit likewise requires fresh semantic facts,
        // while leaving the module/import graph valid for editor queries.
        session
            .set_interface(
                "host.rssi",
                "module host.api\npub fn value() -> Int\npub fn next() -> Int\n",
            )
            .unwrap();
        let signature_edit = session.workspace_module_graph();
        assert!(Arc::ptr_eq(&first, &signature_edit));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 1);

        // Import changes are graph changes and must rebuild instead of serving
        // the stale cached node closure.
        session
            .set_interface(
                "host.rssi",
                "module host.api\nuse host.base\npub fn value() -> Int\n",
            )
            .unwrap();
        let import_edit = session.workspace_module_graph();
        assert!(!Arc::ptr_eq(&first, &import_edit));
        assert_eq!(session.stats().workspace_module_graph_cache_misses, 2);
    }

    #[test]
    fn cached_workspace_diagnostics_obey_cancellation_and_deadline() {
        use std::time::{Duration, Instant};

        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .semantic_workspace_diagnostics_with_operation(&OperationContext::default())
            .unwrap();

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let cancelled_operation = OperationContext {
            cancellation: Some(cancelled),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.semantic_workspace_diagnostics_with_operation(&cancelled_operation),
            Err(OperationAbort::Cancelled)
        ));

        let expired_operation = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.semantic_workspace_diagnostics_with_operation(&expired_operation),
            Err(OperationAbort::DeadlineExceeded)
        ));
        assert_eq!(session.stats().workspace_diagnostic_cache_hits, 0);
    }

    #[test]
    fn source_and_interface_parse_caches_do_not_alias_the_same_file_id() {
        let mut session = CompilationSession::default();
        session
            .set_file("shared.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .set_interface("shared.rssi", "module host.shared\npub fn value() -> Int\n")
            .unwrap();
        let source = session.parse_file("shared.rss").unwrap();
        let interface = session.parse_interface("shared.rssi").unwrap();
        assert!(!Arc::ptr_eq(&source, &interface));
        assert_eq!(session.stats().parse_cache_misses, 2);
    }

    #[test]
    fn session_parse_queries_reject_cancelled_and_expired_requests_before_cache_access() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        let cached = session.parse_file("main.rss").unwrap();

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert_eq!(
            session.parse_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert_eq!(
            session.parse_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );

        let live = OperationContext::default();
        let reused = session
            .parse_file_with_operation("main.rss", &live)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&cached, &reused));
        assert_eq!(session.stats().parse_cache_hits, 1);
    }

    #[test]
    fn compilation_session_caches_hir_by_role_and_immutable_revision() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "fn main() -> Unit { return Unit }")
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn emit() -> Unit\n")
            .unwrap();

        let first = session.hir_file("main.rss").expect("source HIR");
        let second = session.hir_file("main.rss").expect("cached source HIR");
        let interface = session.hir_interface("host.rssi").expect("interface HIR");
        assert!(Arc::ptr_eq(&first, &second));
        assert!(!Arc::ptr_eq(&first, &interface));
        assert_eq!(session.stats().hir_cache_hits, 1);
        assert_eq!(session.stats().hir_cache_misses, 2);

        session
            .set_file("main.rss", "fn main() -> Int { return 1 }")
            .unwrap();
        let replacement = session.hir_file("main.rss").expect("replacement HIR");
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().hir_cache_misses, 3);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.hir_file_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        ));

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.hir_file_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        ));
        assert_eq!(session.stats().hir_cache_misses, 3);
    }

    #[test]
    fn compilation_session_caches_namespace_isolated_workspace_hir() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "module app\nfn helper() -> Int { return 1 }\nfn main() -> Int { return helper() }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();

        let first = session.workspace_hir();
        assert!(first.function_body("app__helper").is_some());
        assert!(first.function_body("main").is_some());
        let second = session.workspace_hir();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_hir_cache_misses, 1);
        assert_eq!(session.stats().workspace_hir_cache_hits, 1);

        session
            .set_interface("host.rssi", "module host\npub fn next() -> Int\n")
            .unwrap();
        let replacement = session.workspace_hir();
        assert!(!Arc::ptr_eq(&first, &replacement));
        assert_eq!(session.stats().workspace_hir_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_hir_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
    }

    #[test]
    fn compilation_session_caches_workspace_type_facts_with_hir_revisions() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "fn main(value: read Int) -> Int { return value }\n",
            )
            .unwrap();
        session
            .set_interface("host.rssi", "module host\npub fn value() -> Int\n")
            .unwrap();

        let analysis = session.workspace_analysis();
        let analysis_types = analysis.database().hir().semantic_types_arc();
        let first = session.workspace_type_facts();
        assert!(first.functions().any(|(name, _)| name == "main"));
        assert!(Arc::ptr_eq(&first, &analysis_types));
        let second = session.workspace_type_facts();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.stats().workspace_type_cache_misses, 1);
        assert_eq!(session.stats().workspace_type_cache_hits, 1);

        session
            .set_interface("host.rssi", "module host\npub fn next() -> Int\n")
            .unwrap();
        let replacement = session.workspace_type_facts();
        assert!(!Arc::ptr_eq(&first, &replacement));
        let replacement_analysis = session.workspace_analysis();
        assert!(Arc::ptr_eq(
            &replacement,
            &replacement_analysis.database().hir().semantic_types_arc()
        ));
        assert_eq!(session.stats().workspace_type_cache_misses, 2);

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert!(matches!(
            session.workspace_type_facts_with_operation(&cancelled),
            Err(OperationAbort::Cancelled)
        ));
        assert_eq!(session.stats().workspace_type_cache_hits, 1);
    }

    #[test]
    fn compilation_session_caches_parsed_module_headers() {
        let mut session = CompilationSession::default();
        session
            .set_file(
                "main.rss",
                "// use ignored.*\nmodule app.core\nuse host.api as host\n",
            )
            .unwrap();
        let first = session.module_header("main.rss").unwrap();
        let second = session.module_header("main.rss").unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.modules(), ["app.core"]);
        assert_eq!(first.imports(), ["host.api"]);
        assert_eq!(
            session.stats(),
            CompilationSessionStats {
                parse_cache_hits: 0,
                parse_cache_misses: 1,
                hir_cache_hits: 0,
                hir_cache_misses: 0,
                workspace_hir_cache_hits: 0,
                workspace_hir_cache_misses: 0,
                workspace_type_cache_hits: 0,
                workspace_type_cache_misses: 0,
                workspace_analysis_cache_hits: 0,
                workspace_analysis_cache_misses: 0,
                module_header_cache_hits: 1,
                module_header_cache_misses: 1,
                workspace_module_graph_cache_hits: 0,
                workspace_module_graph_cache_misses: 0,
                workspace_diagnostic_cache_hits: 0,
                workspace_diagnostic_cache_misses: 0,
                ..CompilationSessionStats::default()
            }
        );
    }

    #[test]
    fn module_header_queries_reject_cancelled_and_expired_cached_requests() {
        let mut session = CompilationSession::default();
        session
            .set_file("main.rss", "module app.core\nuse host.api\n")
            .unwrap();
        session
            .set_interface("host.rssi", "module host.api\npub fn value() -> Unit\n")
            .unwrap();
        let source = session.module_header("main.rss").unwrap();
        let interface = session.interface_module_header("host.rssi").unwrap();

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let cancelled = OperationContext {
            cancellation: Some(cancellation),
            ..OperationContext::default()
        };
        assert_eq!(
            session.module_header_with_operation("main.rss", &cancelled),
            Err(OperationAbort::Cancelled)
        );
        assert_eq!(
            session.interface_module_header_with_operation("host.rssi", &cancelled),
            Err(OperationAbort::Cancelled)
        );

        let expired = OperationContext {
            deadline: Some(MonotonicDeadline::at(
                Instant::now() - Duration::from_millis(1),
            )),
            ..OperationContext::default()
        };
        assert_eq!(
            session.module_header_with_operation("main.rss", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );
        assert_eq!(
            session.interface_module_header_with_operation("host.rssi", &expired),
            Err(OperationAbort::DeadlineExceeded)
        );

        let live = OperationContext::default();
        assert!(Arc::ptr_eq(
            &source,
            &session
                .module_header_with_operation("main.rss", &live)
                .unwrap()
                .unwrap()
        ));
        assert!(Arc::ptr_eq(
            &interface,
            &session
                .interface_module_header_with_operation("host.rssi", &live)
                .unwrap()
                .unwrap()
        ));
    }
