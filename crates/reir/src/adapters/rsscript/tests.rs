#[cfg(test)]
mod tests {
    use super::*;

    fn sample_review_map() -> RsScriptReviewMapInput {
        RsScriptReviewMapInput {
            package_name: "demo_pkg".to_owned(),
            modules: vec![RsScriptModuleInput {
                file: "src/package/review.rss".to_owned(),
                module_path: "rss.package.review".to_owned(),
                line: 1,
                uses: vec![
                    RsScriptUseInput {
                        path: "rss.package.contract.PackageContract".to_owned(),
                        line: 3,
                    },
                    RsScriptUseInput {
                        path: "rss.review.ReviewMap".to_owned(),
                        line: 4,
                    },
                ],
            }],
            regions: vec![
                RsScriptRegionInput {
                    file: "src/lib.rs".to_owned(),
                    function_name: "foldable_fn".to_owned(),
                    classification: RsScriptClassification::Foldable,
                    line: 10,
                    reasons: vec!["pure".to_owned()],
                },
                RsScriptRegionInput {
                    file: "src/lib.rs".to_owned(),
                    function_name: "native_fn".to_owned(),
                    classification: RsScriptClassification::ReviewRequired,
                    line: 22,
                    reasons: vec!["native bridge".to_owned()],
                },
                RsScriptRegionInput {
                    file: "src/lib.rs".to_owned(),
                    function_name: "opaque_fn".to_owned(),
                    classification: RsScriptClassification::Unknown,
                    line: 31,
                    reasons: vec!["macro expansion".to_owned()],
                },
            ],
        }
    }

    fn sample_package_review() -> RsScriptPackageReviewInput {
        RsScriptPackageReviewInput {
            package_name: "demo_pkg".to_owned(),
            version: "1.2.3".to_owned(),
            risk: RsScriptPackageRisk::High,
            features: Vec::new(),
            implements: Vec::new(),
            dependencies: vec![RsScriptPackageDependency {
                name: "rss_core".to_owned(),
                requirement: Some("0.5".to_owned()),
                source: "registry".to_owned(),
                features: vec!["json".to_owned()],
                dependency_kind: "normal".to_owned(),
                compile_only: false,
                test_only: false,
                platform_provided: false,
            }],
            exports: vec![RsScriptPackageExport {
                name: "PackageError".to_owned(),
                kind: "sum_type".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public sum type".to_owned(), "variants: 2".to_owned()],
                normalized_effects: Vec::new(),
            }],
            capabilities: Vec::new(),
            await_sites: Vec::new(),
            diagnostics: Vec::new(),
            public_apis: 8,
            mutating_apis: 2,
            retaining_apis: 1,
            resource_apis: 3,
            native_apis: 2,
            unsafe_apis: 1,
            unknown_apis: 0,
            native_boundaries: vec![RsScriptNativeBoundary {
                module_name: "ffi.crypto".to_owned(),
                functions: vec!["native_fn".to_owned()],
                file: "src/ffi.rs".to_owned(),
                line: 44,
            }],
            native_cargo_features: Vec::new(),
            native_author_declaration: None,
            native_source_scan: None,
        }
    }

    #[test]
    fn review_map_to_facts_skips_foldable_regions() {
        let facts = review_map_to_facts(&sample_review_map());

        assert_eq!(facts.len(), 5);
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::ModuleDeclaration
                && fact.subject.kind == SubjectKind::CodeModule
                && fact.subject.id == "demo_pkg::module::rss.package.review"
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.subject.id == "demo_pkg::module::rss.package.review"
                && fact.evidence[0].symbol.as_deref()
                    == Some("rss.package.contract.PackageContract")
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.subject.id == "demo_pkg::module::rss.package.review"
                && fact.evidence[0].symbol.as_deref() == Some("rss.review.ReviewMap")
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native_fn"
                && fact.evidence[0].file.as_deref() == Some("src/lib.rs")
                && fact.evidence[0].line == Some(22)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::Unknown
                && fact.value == FactValue::Unknown
                && fact.confidence.level == ConfidenceLevel::Unknown
                && fact.subject.id == "demo_pkg::opaque_fn"
        }));
    }

    #[test]
    fn package_review_to_facts_emits_risk_boundary_and_capabilities() {
        let facts = package_review_to_facts(&sample_package_review());

        assert_eq!(facts.len(), 7);
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::PackageRisk
                && fact.subject.kind == SubjectKind::Package
                && fact.subject.id == "demo_pkg@1.2.3"
                && fact.evidence[0].value.as_deref() == Some("high")
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.role == Some(FactRole::Required)
                && fact
                    .capability
                    .as_ref()
                    .map(|capability| capability.category == CapabilityCategory::RuntimeNative)
                    .unwrap_or(false)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.role == Some(FactRole::Required)
                && fact
                    .capability
                    .as_ref()
                    .map(|capability| capability.category == CapabilityCategory::RuntimeUnsafe)
                    .unwrap_or(false)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.kind == SubjectKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native::ffi.crypto"
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::NativeModuleDeclaration
                && fact.subject.kind == SubjectKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native::ffi.crypto"
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.kind == SubjectKind::Package
                && fact.subject.id == "rss_core@0.5"
                && fact.value == FactValue::Unknown
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::PublicContract
                && fact.subject.kind == SubjectKind::CodeType
                && fact.subject.id == "demo_pkg::public::sum_type::PackageError"
                && fact.evidence[0].value.as_deref() == Some("sum_type")
        }));
    }

    #[test]
    fn package_review_to_facts_maps_stdlib_facades_to_capabilities() {
        let mut review = sample_package_review();
        review.exports = vec![
            RsScriptPackageExport {
                name: "Env.get".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Directory.write_string".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Http.get".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Process.run".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Process.run_request".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Process.run_stdout".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Process.run_async".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned(), "async".to_owned()],
            },
            RsScriptPackageExport {
                name: "Process.stream".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Hash.sha256_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Json.parse_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Toml.parse_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Yaml.parse_file".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.open".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.read_all_string".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.read_to_string".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "File.write_buffer".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Csv.open_read".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: Vec::new(),
            },
            RsScriptPackageExport {
                name: "Random.bytes".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
            RsScriptPackageExport {
                name: "Uuid.new_v4".to_owned(),
                kind: "function".to_owned(),
                classification: "review_if_changed".to_owned(),
                reasons: vec!["public function".to_owned()],
                normalized_effects: vec!["native".to_owned()],
            },
        ];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let categories = bundle
            .facts
            .iter()
            .filter_map(|fact| fact.capability.as_ref())
            .map(|capability| capability.category.clone())
            .collect::<Vec<_>>();

        assert!(categories.contains(&CapabilityCategory::EnvRead));
        assert!(categories.contains(&CapabilityCategory::FilesystemWrite));
        assert!(categories.contains(&CapabilityCategory::FilesystemRead));
        assert!(categories.contains(&CapabilityCategory::NetworkClient));
        assert!(categories.contains(&CapabilityCategory::ProcessArgs));
        assert!(categories.contains(&CapabilityCategory::ProcessSpawn));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.subject.id == "demo_pkg::public::function::Process.run"
                && fact.capability.as_ref().is_some_and(|capability| {
                    capability.category == CapabilityCategory::ProcessSpawn
                })
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.subject.id == "demo_pkg::public::function::Process.run_async"
                && fact.capability.as_ref().is_some_and(|capability| {
                    capability.category == CapabilityCategory::ProcessSpawn
                })
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.subject.id == "demo_pkg::public::function::Process.stream"
                && fact.capability.as_ref().is_some_and(|capability| {
                    capability.category == CapabilityCategory::ProcessSpawn
                })
        }));
        assert!(categories.contains(&CapabilityCategory::ComputeHash));
        assert!(categories.contains(&CapabilityCategory::RandomRead));
        assert!(!bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Capability
                && fact.subject.id == "demo_pkg::public::function::File.read_to_string"
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::EnvSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::FilesystemSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NetworkSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::ProcessSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::RandomnessSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_native_source_scan_boundaries() {
        let mut review = sample_package_review();
        review.native_source_scan = Some(RsScriptNativeSourceScan {
            tool: "rss-native-source-scan".to_owned(),
            selected_graph: "package-native-rust".to_owned(),
            worker_thread_parallelism_detected: true,
            unsafe_detected: true,
            ffi_detected: true,
            filesystem_detected: true,
            network_detected: true,
            build_script_present: true,
        });

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let categories = bundle
            .facts
            .iter()
            .filter_map(|fact| fact.capability.as_ref())
            .map(|capability| capability.category.clone())
            .collect::<Vec<_>>();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::UnsafeBoundary
                && fact.subject.kind == SubjectKind::UnsafeBoundary
                && fact.confidence.level == ConfidenceLevel::Scanned
                && fact.acquisition_mode == AcquisitionMode::SourceScan
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "demo_pkg::native::ffi"
                && fact.confidence.level == ConfidenceLevel::Scanned
        }));
        assert!(categories.contains(&CapabilityCategory::RuntimeUnsafe));
        assert!(categories.contains(&CapabilityCategory::RuntimeNative));
        assert!(categories.contains(&CapabilityCategory::FilesystemRead));
        assert!(categories.contains(&CapabilityCategory::NetworkClient));
        assert!(categories.contains(&CapabilityCategory::ProcessSpawn));
        assert!(categories.contains(&CapabilityCategory::BuildExecute));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::BuildTimeExecution
                && fact.subject.kind == SubjectKind::BuildStep
                && fact.subject.id == "demo_pkg@1.2.3::build::native_rust_build_script"
                && fact.confidence.level == ConfidenceLevel::Scanned
                && fact.acquisition_mode == AcquisitionMode::SourceScan
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NativeUnsafeSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::FilesystemSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NetworkSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::ProcessSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::BuildTimeSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_await_sites_as_async_boundaries() {
        let mut review = sample_package_review();
        review.await_sites = vec![RsScriptPackageAwaitSite {
            function: "Api.run".to_owned(),
            callee: Some("Timer.sleep".to_owned()),
            boundary: "native_pending".to_owned(),
            live_across_await: vec!["client".to_owned()],
            file: "src/api.rss".to_owned(),
            line: 12,
            column: 9,
        }];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let fact = bundle
            .facts
            .iter()
            .find(|fact| fact.kind == FactKind::AsyncBoundary)
            .expect("async boundary fact");

        assert_eq!(fact.subject.id, "demo_pkg::Api.run");
        assert_eq!(fact.confidence.level, ConfidenceLevel::Authoritative);
        assert_eq!(fact.acquisition_mode, AcquisitionMode::CompilerContract);
        assert_eq!(fact.evidence[0].file.as_deref(), Some("src/api.rss"));
        assert_eq!(fact.evidence[0].line, Some(12));
        assert_eq!(fact.evidence[0].column, Some(9));
        assert!(
            fact.evidence[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("boundary=native_pending"))
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::AsyncSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_diagnostics() {
        let mut review = sample_package_review();
        review.diagnostics = vec![RsScriptDiagnosticInput {
            code: "RS0015".to_owned(),
            severity: "error".to_owned(),
            summary: "unsupported syntax".to_owned(),
            spans: vec![RsScriptDiagnosticSpan {
                file: "interface/lib.rssi".to_owned(),
                line: 1,
                column: 4,
                length: 2,
                label: "unsupported".to_owned(),
            }],
        }];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let fact = bundle
            .facts
            .iter()
            .find(|fact| fact.kind == FactKind::Diagnostic)
            .expect("diagnostic fact");

        assert_eq!(fact.subject.kind, SubjectKind::CodeFile);
        assert_eq!(fact.subject.id, "demo_pkg::interface/lib.rssi");
        assert_eq!(fact.value, FactValue::Unknown);
        assert_eq!(fact.evidence[0].symbol.as_deref(), Some("RS0015"));
        assert_eq!(fact.evidence[0].file.as_deref(), Some("interface/lib.rssi"));
        assert_eq!(fact.evidence[0].line, Some(1));
        assert_eq!(fact.evidence[0].column, Some(4));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::DiagnosticSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_features_and_provider_implementations() {
        let mut review = sample_package_review();
        review.features = vec!["native-tls".to_owned()];
        review.implements = vec![RsScriptProviderImplementation {
            interface_package: "platform-env".to_owned(),
            version: Some("0.1".to_owned()),
            interface_features: vec!["posix".to_owned()],
            interface_effective_hash: Some("sha256:abc".to_owned()),
        }];

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PackageFeature
                && fact.subject.kind == SubjectKind::PackageFeature
                && fact.subject.id == "demo_pkg@1.2.3#feature:native-tls"
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::ProviderImplementation
                && fact.subject.id == "demo_pkg@1.2.3::implements::platform-env"
                && fact.value == FactValue::True
                && fact.evidence[0]
                    .reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("interface_effective_hash=sha256:abc"))
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::PackageFeatureSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::ProviderImplementationSlice)
        );
    }

    #[test]
    fn package_review_to_facts_preserves_native_author_and_cargo_feature_metadata() {
        let mut review = sample_package_review();
        review.native_cargo_features = vec!["base-native".to_owned()];
        review.native_author_declaration = Some(RsScriptNativeAuthorDeclaration {
            worker_thread_parallelism: true,
            native_parallel_backend: Some("worker-pool".to_owned()),
            risk_reasons: vec!["native API declares parallel worker execution".to_owned()],
        });

        let bundle = rsscript_to_bundle(&sample_review_map(), &review);
        let author_fact = bundle
            .facts
            .iter()
            .find(|fact| {
                fact.capability.as_ref().is_some_and(|capability| {
                    capability.service.as_deref() == Some("native_rust_author_declaration")
                })
            })
            .expect("native author declaration capability fact");

        assert_eq!(author_fact.confidence.level, ConfidenceLevel::Declared);
        assert_eq!(
            author_fact.acquisition_mode,
            AcquisitionMode::ManualDeclaration
        );
        assert_eq!(
            author_fact
                .capability
                .as_ref()
                .map(|capability| capability.category.clone()),
            Some(CapabilityCategory::ProcessSpawn)
        );
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeCargoFeature
                && fact.subject.id == "demo_pkg@1.2.3#native-cargo-feature:base-native"
        }));
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NativeUnsafeSlice)
        );
    }

    #[test]
    fn package_lock_to_facts_emits_lockfile_supply_chain_hashes() {
        let lock = RsScriptPackageLockInput {
            version: 1,
            lockfile_path: Some("/tmp/demo/rsspkg.lock".to_owned()),
            packages: vec![RsScriptPackageLockPackage {
                name: "demo_pkg".to_owned(),
                version: "1.2.3".to_owned(),
                source: "path+/tmp/demo".to_owned(),
                checksum: "sha256:pkg".to_owned(),
                interface_hash: "sha256:iface".to_owned(),
                review_hash: "sha256:review".to_owned(),
                native_hash: Some("sha256:native".to_owned()),
                features: vec!["fast".to_owned()],
            }],
        };

        let bundle = {
            let json = serde_json::to_string(&lock).unwrap();
            rsscript_lock_json_to_bundle(&json).unwrap()
        };

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.lockfile.demo_pkg_1_2_3.effective_interface_hash"
                && fact.acquisition_mode == AcquisitionMode::Lockfile
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package/0/interface_hash")
                && fact.evidence[0].value.as_deref() == Some("sha256:iface")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.lockfile.demo_pkg_1_2_3.native_hash"
                && fact.evidence[0].value.as_deref() == Some("sha256:native")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.lockfile.demo_pkg_1_2_3.review_hash".to_owned())
        }));
    }

    #[test]
    fn package_lock_to_facts_marks_missing_hashes_unknown() {
        let lock = RsScriptPackageLockInput {
            version: 1,
            lockfile_path: Some("/tmp/demo/rsspkg.lock".to_owned()),
            packages: vec![RsScriptPackageLockPackage {
                name: "demo_pkg".to_owned(),
                version: "1.2.3".to_owned(),
                source: "path+/tmp/demo".to_owned(),
                checksum: String::new(),
                interface_hash: String::new(),
                review_hash: String::new(),
                native_hash: Some(String::new()),
                features: Vec::new(),
            }],
        };

        let bundle = {
            let json = serde_json::to_string(&lock).unwrap();
            rsscript_lock_json_to_bundle(&json).unwrap()
        };

        for id in [
            "fact.lockfile.demo_pkg_1_2_3.checksum",
            "fact.lockfile.demo_pkg_1_2_3.effective_interface_hash",
            "fact.lockfile.demo_pkg_1_2_3.review_hash",
            "fact.lockfile.demo_pkg_1_2_3.native_hash",
        ] {
            let fact = bundle
                .facts
                .iter()
                .find(|fact| fact.id == id)
                .unwrap_or_else(|| panic!("missing lockfile fact `{id}`"));
            assert_eq!(fact.kind, FactKind::SupplyChain);
            assert_eq!(fact.value, FactValue::Unknown);
            assert!(fact.evidence[0].value.is_none());
            assert!(
                fact.unknown_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("is missing"))
            );
        }
    }

    #[test]
    fn package_check_to_bundle_emits_gate_policy_facts() {
        let check = RsScriptPackageCheckInput {
            package: RsScriptPackageIdentityInput {
                name: "demo".to_owned(),
                version: "0.1.0".to_owned(),
                edition: "2026".to_owned(),
            },
            package_dir: "/tmp/demo".to_owned(),
            ok: false,
            risk: RsScriptPackageRisk::High,
            reasons: vec!["rsspkg.lock missing".to_owned()],
            summary: RsScriptPackageCheckSummary {
                diagnostics: 1,
                errors: 1,
                dependencies: 1,
                native_apis: 1,
                unsafe_apis: 1,
                unknown_apis: 0,
            },
            graph: RsScriptPackageGraphCheckInput {
                ok: true,
                risk: RsScriptPackageRisk::Low,
                reasons: Vec::new(),
            },
            lock: RsScriptPackageCheckLockInput {
                path: "/tmp/demo/rsspkg.lock".to_owned(),
                present: false,
                matches: false,
                risk: RsScriptPackageRisk::Elevated,
                reasons: vec!["rsspkg.lock missing".to_owned()],
                package_changes: Vec::new(),
            },
            implements: vec![RsScriptProviderImplementation {
                interface_package: "platform-env".to_owned(),
                version: Some("0.1".to_owned()),
                interface_features: vec!["posix".to_owned()],
                interface_effective_hash: None,
            }],
            native_rust: Some(RsScriptPackageNativeRustCheckInput {
                path: "/tmp/demo/native".to_owned(),
                cargo_toml_present: true,
                cargo_metadata_ok: true,
                cargo_package_name: Some("demo-native".to_owned()),
                target_kinds: vec!["lib".to_owned()],
                unsafe_detected: true,
                linked_libraries: Vec::new(),
                build_env_detected: true,
                build_download_detected: false,
                file_count: 2,
                ok: false,
                risk: RsScriptPackageRisk::High,
                reasons: vec!["native Rust unsafe usage".to_owned()],
            }),
            diagnostics: vec![RsScriptDiagnosticInput {
                code: "PKG0601".to_owned(),
                severity: "error".to_owned(),
                summary: "native policy violation".to_owned(),
                spans: vec![RsScriptDiagnosticSpan {
                    file: "rsspkg.toml".to_owned(),
                    line: 3,
                    column: 1,
                    length: 4,
                    label: "native".to_owned(),
                }],
            }],
        };

        let json = serde_json::to_string(&check).unwrap();
        let bundle = rsscript_check_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.status"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo")
                && fact.evidence[0].json_pointer.as_deref() == Some("/ok")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.graph"
                && fact.acquisition_mode == AcquisitionMode::PackageMetadata
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo")
                && fact.evidence[0].json_pointer.as_deref() == Some("/graph")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.lock"
                && fact.acquisition_mode == AcquisitionMode::Lockfile
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/lock")
                && fact.evidence[0].value.as_deref() == Some("elevated")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.package_check.demo_0_1_0.native"
                && fact.acquisition_mode == AcquisitionMode::PackageMetadata
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/native")
                && fact.evidence[0].json_pointer.as_deref() == Some("/native_rust")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::UnsafeBoundary
                && fact.id == "fact.package_check.demo_0_1_0.unsafe"
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/native")
                && fact.evidence[0].json_pointer.as_deref() == Some("/native_rust/unsafe_detected")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::BuildTimeExecution
                && fact.id == "fact.package_check.demo_0_1_0.build_time"
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/native")
                && fact.evidence[0].json_pointer.as_deref() == Some("/native_rust")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Diagnostic
                && fact.id.contains("PKG0601")
                && fact.evidence[0].source.as_deref() == Some(PACKAGE_CHECK_SOURCE)
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.toml")
                && fact.evidence[0].line == Some(3)
                && fact.evidence[0].column == Some(1)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::ProviderImplementation
                && fact.id == "fact.package_check.demo_0_1_0.provider_implementation.platform_env"
                && fact.subject.id == "demo@0.1.0::implements::platform-env"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].source.as_deref() == Some(PACKAGE_CHECK_SOURCE)
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/rsspkg.toml")
                && fact.evidence[0].json_pointer.as_deref() == Some("/implements/0")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::DiagnosticSlice
                && slice.facts.iter().any(|fact| fact.contains("PKG0601"))
        }));
    }

    #[test]
    fn package_lock_diff_to_bundle_emits_update_review_facts() {
        let diff = RsScriptPackageLockDiffInput {
            old_lock_path: "old.rsspkg.lock".to_owned(),
            new_lock_path: "new.rsspkg.lock".to_owned(),
            risk: RsScriptPackageRisk::High,
            reasons: vec![".rssi interface hash changed".to_owned()],
            old_packages: 1,
            new_packages: 1,
            package_changes: vec![
                RsScriptPackageLockPackageChange {
                    name: "dep".to_owned(),
                    before_version: Some("0.1.0".to_owned()),
                    after_version: Some("0.2.0".to_owned()),
                    risk: RsScriptPackageRisk::High,
                    changes: vec![RsScriptPackageLockFieldChange {
                        field: "interface_hash".to_owned(),
                        before: Some("sha256:old".to_owned()),
                        after: Some("sha256:new".to_owned()),
                        risk: RsScriptPackageRisk::High,
                    }],
                },
                RsScriptPackageLockPackageChange {
                    name: "old-dep".to_owned(),
                    before_version: Some("0.1.0".to_owned()),
                    after_version: None,
                    risk: RsScriptPackageRisk::Elevated,
                    changes: vec![RsScriptPackageLockFieldChange {
                        field: "package".to_owned(),
                        before: Some("removed".to_owned()),
                        after: None,
                        risk: RsScriptPackageRisk::Elevated,
                    }],
                },
            ],
        };

        let json = serde_json::to_string(&diff).unwrap();
        let bundle = rsscript_lock_diff_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id.contains("field.interface_hash")
                && fact.subject.id == "dep@0.2.0"
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("new.rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package_changes/0/changes/0")
                && fact.evidence[0].value.as_deref() == Some("sha256:new")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.id.contains("old_dep")
                && fact.subject.id == "old-dep@0.1.0"
                && fact.evidence[0].kind == EvidenceKind::LockfileEntry
                && fact.evidence[0].file.as_deref() == Some("old.rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package_changes/1")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.id.contains("field.package")
                && fact.subject.id == "old-dep@0.1.0"
                && fact.evidence[0].file.as_deref() == Some("old.rsspkg.lock")
                && fact.evidence[0].json_pointer.as_deref() == Some("/package_changes/1/changes/0")
                && fact.evidence[0].value.as_deref() == Some("removed")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id.contains(".risk")
                && fact.evidence[0].json_pointer.as_deref() == Some("/risk")
                && fact.evidence[0].resource.as_deref()
                    == Some("old.rsspkg.lock -> new.rsspkg.lock")
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .iter()
                    .any(|fact| fact.contains("interface_hash"))
        }));
    }

    #[test]
    fn package_tree_to_bundle_emits_dependency_path_facts_and_edges() {
        let tree = RsScriptPackageTreeInput {
            root: RsScriptPackageTreeNode {
                name: "app".to_owned(),
                version: Some("0.1.0".to_owned()),
                requirement: None,
                source: "path+/tmp/app".to_owned(),
                risk: RsScriptPackageRisk::Low,
                features: Vec::new(),
                native: false,
                interface_only: false,
                compile_only: false,
                test_only: false,
                platform_provided: false,
                interface_effective_hash: "sha256:app".to_owned(),
                implements: Vec::new(),
                dependency_kind: "root".to_owned(),
                reasons: Vec::new(),
                dependencies: vec![
                    RsScriptPackageTreeNode {
                        name: "dep".to_owned(),
                        version: Some("1.0.0".to_owned()),
                        requirement: Some("^1".to_owned()),
                        source: "path+/tmp/dep".to_owned(),
                        risk: RsScriptPackageRisk::Elevated,
                        features: vec!["fast".to_owned()],
                        native: true,
                        interface_only: false,
                        compile_only: false,
                        test_only: false,
                        platform_provided: false,
                        interface_effective_hash: "sha256:dep".to_owned(),
                        implements: Vec::new(),
                        dependency_kind: "normal".to_owned(),
                        reasons: vec!["native API".to_owned()],
                        dependencies: Vec::new(),
                    },
                    RsScriptPackageTreeNode {
                        name: "registry-dep".to_owned(),
                        version: None,
                        requirement: Some("^2".to_owned()),
                        source: "registry".to_owned(),
                        risk: RsScriptPackageRisk::Unknown,
                        features: Vec::new(),
                        native: false,
                        interface_only: false,
                        compile_only: false,
                        test_only: false,
                        platform_provided: false,
                        interface_effective_hash: String::new(),
                        implements: Vec::new(),
                        dependency_kind: "normal".to_owned(),
                        reasons: vec!["dependency resolver not implemented".to_owned()],
                        dependencies: Vec::new(),
                    },
                    RsScriptPackageTreeNode {
                        name: "missing-path-dep".to_owned(),
                        version: None,
                        requirement: None,
                        source: "path+/tmp/missing-path-dep".to_owned(),
                        risk: RsScriptPackageRisk::Unknown,
                        features: Vec::new(),
                        native: false,
                        interface_only: false,
                        compile_only: false,
                        test_only: false,
                        platform_provided: false,
                        interface_effective_hash: String::new(),
                        implements: Vec::new(),
                        dependency_kind: "normal".to_owned(),
                        reasons: vec!["path dependency manifest missing".to_owned()],
                        dependencies: Vec::new(),
                    },
                ],
            },
            summary: RsScriptPackageTreeSummary::default(),
        };

        let json = serde_json::to_string(&tree).unwrap();
        let bundle = rsscript_tree_json_to_bundle(&json).unwrap();

        assert!(bundle.producers.iter().any(|producer| {
            producer.adapter.as_deref() == Some("rsscript-package-tree")
                && producer.source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "dep@1.0.0"
                && fact.confidence.source.as_deref() == Some(TREE_SOURCE)
                && fact.evidence[0].kind == EvidenceKind::DependencyPath
                && fact.evidence[0].file.as_deref() == Some("/tmp/dep")
                && fact.evidence[0].json_pointer.as_deref() == Some("/root/dependencies/0")
                && fact.evidence[0].source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "registry-dep@^2"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.is_none()
                && fact.evidence[0].json_pointer.as_deref() == Some("/root/dependencies/1")
                && fact.unknown_reason.as_deref() == Some("dependency resolver not implemented")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::DependencyRisk
                && fact.subject.id == "missing-path-dep@path+/tmp/missing-path-dep"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].file.is_none()
                && fact.evidence[0].json_pointer.as_deref() == Some("/root/dependencies/2")
                && fact.unknown_reason.as_deref() == Some("path dependency manifest missing")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.package_tree.root_1.effective_interface_hash"
                && fact.evidence[0].file.as_deref() == Some("/tmp/dep")
                && fact.evidence[0].value.as_deref() == Some("sha256:dep")
                && fact.evidence[0].source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.edges.iter().any(|edge| {
            edge.kind == EdgeKind::DependsOn
                && edge.from.id == "app@0.1.0"
                && edge.to.id == "dep@1.0.0"
                && edge.confidence.source.as_deref() == Some(TREE_SOURCE)
                && edge.evidence[0].kind == EvidenceKind::DependencyPath
                && edge.evidence[0].file.as_deref() == Some("/tmp/dep")
                && edge.evidence[0].source.as_deref() == Some(TREE_SOURCE)
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.package_tree.root_1.risk".to_owned())
        }));
    }

       #[test]
    fn package_metadata_to_bundle_emits_artifact_status_facts() {
        let metadata = RsScriptPackageMetadataInput {
            package: RsScriptPackageIdentityInput {
                name: "demo".to_owned(),
                version: "0.1.0".to_owned(),
                edition: "2026".to_owned(),
            },
            package_dir: "/tmp/demo".to_owned(),
            metadata_path: "/tmp/demo/review/package-review.json".to_owned(),
            reir_path: "/tmp/demo/review/reir/rsscript.json".to_owned(),
            dry_run: false,
            written: false,
            verified: false,
            ok: false,
            risk: RsScriptPackageRisk::Unknown,
            reasons: vec!["unknown API".to_owned()],
            mismatches: vec![RsScriptPackageMetadataMismatch {
                artifact: "reir_bundle".to_owned(),
                path: "/tmp/demo/review/reir/rsscript.json".to_owned(),
                kind: "stale".to_owned(),
                message: "artifact does not match the current package review result".to_owned(),
                expected_sha256: "sha256:expected".to_owned(),
                actual_sha256: Some("sha256:actual".to_owned()),
            }],
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let bundle = rsscript_metadata_json_to_bundle(&json).unwrap();

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id == "fact.metadata.demo_0_1_0.status"
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo")
                && fact.evidence[0].json_pointer.as_deref() == Some("/ok")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::SupplyChain
                && fact.id == "fact.metadata.demo_0_1_0.reir_artifact"
                && fact.value == FactValue::Unknown
                && fact.evidence[0].kind == EvidenceKind::PackageMetadata
                && fact.evidence[0].json_pointer.as_deref() == Some("/reir_path")
                && fact.evidence[0].value.as_deref() == Some("/tmp/demo/review/reir/rsscript.json")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PolicyResult
                && fact.id.contains(".mismatch.")
                && fact.evidence[0].file.as_deref() == Some("/tmp/demo/review/reir/rsscript.json")
                && fact.evidence[0].json_pointer.as_deref() == Some("/mismatches/0")
                && fact.evidence[0].value.as_deref().is_some_and(|value| {
                    value.contains("expected=sha256:expected")
                        && value.contains("actual=sha256:actual")
                })
                && fact.unknown_reason.as_deref().is_some_and(|reason| {
                    reason.contains("review/reir/rsscript.json")
                        && reason.contains("stale")
                        && reason.contains("sha256:expected")
                        && reason.contains("sha256:actual")
                })
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.metadata.demo_0_1_0.reir_artifact".to_owned())
        }));
    }

     #[test]
    fn rsscript_bundle_serializes_round_trip() {
        let review_map = sample_review_map();
        let package_review = sample_package_review();

        let bundle = rsscript_to_bundle(&review_map, &package_review);
        let json = bundle.to_json().unwrap();
        let round_trip = Bundle::from_json(&json).unwrap();

        assert_eq!(bundle.producers.len(), 1);
        assert_eq!(bundle.producers[0].name, "rssc");
        assert_eq!(
            bundle.producers[0].adapter.as_deref(),
            Some("rsscript-language")
        );
        assert_eq!(bundle.producers[0].adapter_version.as_deref(), Some("0.1"));
        assert_eq!(
            bundle.producers[0].source.as_deref(),
            Some("compiler_contract")
        );
        assert_eq!(bundle.facts.len(), 12);
        assert_eq!(bundle.edges.len(), 3);
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::PackageRiskSlice)
        );
        assert!(
            bundle
                .slices
                .iter()
                .any(|slice| slice.kind == SliceKind::NativeUnsafeSlice)
        );
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::Package && subject.id == "demo_pkg@1.2.3"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::CodeModule
                && subject.id == "demo_pkg::module::rss.package.review"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::NativeBoundary
                && subject.id == "demo_pkg::native::ffi.crypto"
        }));
        assert!(bundle.edges.iter().any(|edge| {
            edge.kind == EdgeKind::NormalizesToNativeFn
                && edge.from.id == "demo_pkg::native::ffi.crypto"
                && edge.to.id == "demo_pkg::native_fn"
        }));
        assert_eq!(round_trip, bundle);
    }

    #[test]
    fn rsscript_json_collects_package_review_with_embedded_review_map() {
        let package_review = r#"{
            "package": { "name": "rss-native-sample", "version": "0.1.0" },
            "risk": "high",
            "summary": {
                "public_apis": 4,
                "mutating_apis": 1,
                "retaining_apis": 0,
                "resource_apis": 0,
                "native_apis": 4,
                "unsafe_apis": 0,
                "unknown_apis": 0
            },
            "files": [
                { "path": "packages/native-sample/interface/lib.rssi", "kind": "interface" }
            ],
            "exports": [
                {
                    "name": "NativeSample.sum_int",
                    "kind": "function",
                    "classification": "review_if_changed",
                    "normalized_effects": ["native", "parallel"]
                },
                {
                    "name": "NativeSample.sort_int",
                    "kind": "function",
                    "classification": "review_if_changed",
                    "normalized_effects": ["native", "parallel"]
                }
            ],
            "review_map": {
                "files": [
                    {
                        "file": "packages/native-sample/interface/lib.rssi",
                        "regions": [
                            {
                                "function": "NativeSample.sum_int",
                                "classification": "must_review",
                                "line": 3,
                                "reasons": ["native boundary", "parallel boundary"]
                            },
                            {
                                "function": "NativeSample.sort_int",
                                "classification": "must_review",
                                "line": 7,
                                "reasons": ["native boundary", "parallel boundary"]
                            }
                        ]
                    }
                ]
            }
        }"#;

        let bundle = rsscript_json_to_bundle(None, Some(package_review), None).unwrap();

        assert_eq!(bundle.producers.len(), 1);
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PackageRisk && fact.subject.id == "rss-native-sample@0.1.0"
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "rss-native-sample::NativeSample.sum_int"
                && fact.evidence[0].line == Some(3)
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::PublicContract
                && fact.subject.kind == SubjectKind::CodePublicApi
                && fact.subject.id == "rss-native-sample::public::function::NativeSample.sum_int"
                && fact.evidence[0].value.as_deref() == Some("function")
        }));
        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::NativeBoundary
                && fact.subject.id == "rss-native-sample::native::NativeSample"
                && fact.evidence[0].file.as_deref()
                    == Some("packages/native-sample/interface/lib.rssi")
                && fact.evidence[0].line == Some(3)
        }));
        assert_eq!(
            bundle
                .facts
                .iter()
                .filter(|fact| {
                    fact.id == "fact.native_boundary.rss_native_sample__native__NativeSample"
                        && fact.kind == FactKind::NativeBoundary
                })
                .count(),
            1
        );
        assert!(bundle.edges.iter().any(|edge| {
            edge.from.id == "rss-native-sample::NativeSample.sum_int"
                && edge.to.id == "rss-native-sample::native::NativeSample"
        }));
        assert!(bundle.edges.iter().any(|edge| {
            edge.from.id == "rss-native-sample::NativeSample.sort_int"
                && edge.to.id == "rss-native-sample::native::NativeSample"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::Package && subject.id == "rss-native-sample@0.1.0"
        }));
        assert!(bundle.subjects.iter().any(|subject| {
            subject.kind == SubjectKind::NativeBoundary
                && subject.id == "rss-native-sample::native::NativeSample"
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::NativeUnsafeSlice
                && slice
                    .facts
                    .contains(
                        &"fact.native_boundary.rss_native_sample__native__NativeSample".to_owned(),
                    )
        }));
        assert!(bundle.slices.iter().any(|slice| {
            slice.kind == SliceKind::PackageRiskSlice
                && slice
                    .facts
                    .contains(&"fact.package.rss_native_sample_0_1_0.risk".to_owned())
        }));
    }

    #[test]
    fn review_map_json_import_preserves_module_and_use_declarations() {
        let json = r#"{
            "summary": {},
            "modules": [
                {
                    "file": "src/package/review.rss",
                    "module_path": "rss.package.review",
                    "line": 3,
                    "uses": [
                        { "path": "rss.package.contract.PackageContract", "line": 5 },
                        { "path": "rss.review.ReviewMap", "line": 6 }
                    ]
                }
            ],
            "files": []
        }"#;

        let input =
            review_map_input_from_json("demo_pkg", json).expect("review map JSON should import");
        let facts = review_map_to_facts(&input);

        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::ModuleDeclaration
                && fact.subject.id == "demo_pkg::module::rss.package.review"
                && fact.evidence[0].line == Some(3)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.evidence[0].symbol.as_deref()
                    == Some("rss.package.contract.PackageContract")
                && fact.evidence[0].line == Some(5)
        }));
        assert!(facts.iter().any(|fact| {
            fact.kind == FactKind::UseDeclaration
                && fact.evidence[0].symbol.as_deref() == Some("rss.review.ReviewMap")
                && fact.evidence[0].line == Some(6)
        }));
    }

    #[test]
    fn rsscript_json_collects_review_map_only_with_explicit_package_name() {
        let review_map = r#"{
            "summary": { "total_functions": 2 },
            "files": [
                {
                    "file": "tests/fixtures/pass/module-use-basic.rss",
                    "regions": [
                        {
                            "function": "review_package",
                            "classification": "low_semantic_risk",
                            "line": 7,
                            "reasons": ["private pure helper"]
                        },
                        {
                            "function": "main",
                            "classification": "must_review",
                            "line": 11,
                            "reasons": ["entry point"]
                        }
                    ]
                }
            ]
        }"#;

        let bundle = rsscript_json_to_bundle(Some(review_map), None, Some("pkg_tool")).unwrap();

        assert_eq!(bundle.facts.len(), 1);
        let fact = &bundle.facts[0];
        assert_eq!(
            fact.kind,
            FactKind::Extension(REVIEW_REQUIRED_KIND.to_owned())
        );
        assert_eq!(fact.subject.id, "pkg_tool::main");
        assert_eq!(fact.evidence[0].line, Some(11));
        assert_eq!(bundle.subjects.len(), 1);
        assert_eq!(bundle.subjects[0].id, "pkg_tool::main");
        assert!(bundle.slices.is_empty());
    }
}
