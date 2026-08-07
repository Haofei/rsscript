#[cfg(test)]
mod tests {
    use super::*;

    fn sample_package_analysis_json() -> &'static str {
        r#"{
            "$schema": "rsscript.package_analysis.v1",
            "producer": {
                "name": "rsscript",
                "version": "0.1.0",
                "source_revision": "abc123",
                "ruleset_digest": "sha256:rules"
            },
            "language_version": "2026",
            "interface_catalog_digest": "sha256:interfaces",
            "snapshot_digest": "sha256:snapshot",
            "module_digest": "sha256:module",
            "package": { "name": "demo_pkg", "version": "1.2.3", "edition": "2026" },
            "files": [],
            "summary": {},
            "exports": [
                {
                    "name": "Api.transform",
                    "kind": "function",
                    "function_kind": "async",
                    "retained_params": ["input"],
                    "semantic_facts": [
                        "async boundary",
                        "mut parameter `state`",
                        "resource boundary",
                        "returns fresh value",
                        "retains(input)"
                    ]
                }
            ],
            "external_imports": [
                {
                    "function": "Api.transform",
                    "symbol": "host.data.load",
                    "call_chain": ["Api.transform", "host.data.load"],
                    "span": { "file": "src/lib.rss", "line": 8, "column": 5, "length": 14 }
                }
            ],
            "await_sites": [
                {
                    "function": "Api.transform",
                    "callee": "host.data.load",
                    "live_across_await": ["state"],
                    "span": { "file": "src/lib.rss", "line": 8, "column": 5, "length": 14 }
                }
            ],
            "diagnostics": []
        }"#
    }

    #[test]
    fn package_analysis_is_neutral_digest_bound_reir_evidence() {
        let bundle = rsscript_analysis_json_to_bundle(sample_package_analysis_json())
            .expect("neutral package analysis should convert");

        assert!(bundle.facts.iter().any(|fact| {
            fact.kind == FactKind::Extension("package_analysis".to_owned())
                && fact.evidence[0]
                    .reason
                    .as_deref()
                    .is_some_and(|reason| {
                        reason.contains("snapshot=sha256:snapshot")
                            && reason.contains("module=sha256:module")
                    })
        }));
        for kind in [
            FactKind::PublicContract,
            FactKind::Retention,
            FactKind::Mutation,
            FactKind::Resource,
            FactKind::AsyncBoundary,
            FactKind::Extension("freshness".to_owned()),
            FactKind::Extension("external_import".to_owned()),
        ] {
            assert!(
                bundle.facts.iter().any(|fact| fact.kind == kind),
                "missing package-analysis fact kind {kind:?}"
            );
        }
        assert!(
            !bundle
                .facts
                .iter()
                .any(|fact| fact.kind == FactKind::Capability),
            "external symbols are not capability evidence without binding/provider metadata"
        );
        assert_eq!(
            bundle.producers[0].source.as_deref(),
            Some(PACKAGE_ANALYSIS_SOURCE)
        );
    }

    #[test]
    fn package_analysis_rejects_unknown_or_extended_schema() {
        let unknown = sample_package_analysis_json().replace(
            "rsscript.package_analysis.v1",
            "rsscript.package_analysis.v2",
        );
        let error = rsscript_analysis_json_to_bundle(&unknown)
            .expect_err("unknown package-analysis schema must fail closed");
        assert!(
            error
                .to_string()
                .contains("unsupported RSScript package analysis schema")
        );

        let extended = sample_package_analysis_json().replacen(
            "\"language_version\"",
            "\"unexpected\": true, \"language_version\"",
            1,
        );
        let error = rsscript_analysis_json_to_bundle(&extended)
            .expect_err("unknown package-analysis fields must fail closed");
        assert!(error.to_string().contains("unknown field `unexpected`"));
    }
}
