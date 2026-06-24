//! REIR bundle bridge for the Behavior Bill of Materials (BBOM).
//!
//! Converts between [`BehaviorBom`] and REIR [`reir::Bundle`] so that .rssi
//! contract files can produce REIR evidence (and vice versa) without full
//! RSScript package compilation.

use crate::bbom::{
    BehaviorBom, BomBoundaryKind, BomCapability, BomMutation, BomMutationKind, BomNativeBoundary,
    BomResource, BomRetention, BomRetentionKind, BomSummary, BomUnknown, BomUnknownFunction,
};

/// Convert a BehaviorBom to a REIR Bundle for CI integration.
/// This allows .rssi contract files to produce REIR evidence without
/// full RSScript package compilation.
pub fn behavior_bom_to_reir_bundle(bom: &BehaviorBom, source_paths: &[&str]) -> reir::Bundle {
    let mut facts = Vec::new();
    let source_label = if source_paths.len() == 1 {
        source_paths[0].to_string()
    } else {
        format!("{} contracts", source_paths.len())
    };

    for m in &bom.mutations {
        facts.push(reir::Fact {
            schema: "reir.fact.v0.2".to_owned(),
            id: format!(
                "contract-mut-{}-{}",
                sanitize(&m.function),
                sanitize(&m.target)
            ),
            kind: reir::FactKind::Mutation,
            role: Some(reir::FactRole::Required),
            subject: reir::Subject {
                kind: reir::SubjectKind::CodeFunction,
                id: m.function.clone(),
                name: Some(m.function.clone()),
                package: None,
            },
            capability: None,
            value: reir::FactValue::True,
            evidence: vec![contract_evidence(
                &source_label,
                Some(&m.function),
                &format!("mut parameter: {}", m.target),
            )],
            confidence: reir::Confidence {
                level: reir::ConfidenceLevel::Declared,
                source: Some("rssi-contract".to_owned()),
            },
            acquisition_mode: reir::AcquisitionMode::NormalizedInterface,
            precision: reir::Precision::Exact,
            unknown_reason: None,
        });
    }

    for r in &bom.retentions {
        facts.push(reir::Fact {
            schema: "reir.fact.v0.2".to_owned(),
            id: format!(
                "contract-retain-{}-{}",
                sanitize(&r.function),
                sanitize(&r.parameter)
            ),
            kind: reir::FactKind::Retention,
            role: Some(reir::FactRole::Required),
            subject: reir::Subject {
                kind: reir::SubjectKind::CodeFunction,
                id: r.function.clone(),
                name: Some(r.function.clone()),
                package: None,
            },
            capability: None,
            value: reir::FactValue::True,
            evidence: vec![contract_evidence(
                &source_label,
                Some(&r.function),
                &format!("retains: {}", r.parameter),
            )],
            confidence: reir::Confidence {
                level: reir::ConfidenceLevel::Declared,
                source: Some("rssi-contract".to_owned()),
            },
            acquisition_mode: reir::AcquisitionMode::NormalizedInterface,
            precision: reir::Precision::Exact,
            unknown_reason: None,
        });
    }

    for res in &bom.resources {
        facts.push(reir::Fact {
            schema: "reir.fact.v0.2".to_owned(),
            id: format!("contract-resource-{}", sanitize(&res.kind)),
            kind: reir::FactKind::Resource,
            role: Some(reir::FactRole::Required),
            subject: reir::Subject {
                kind: reir::SubjectKind::CodeType,
                id: res.kind.clone(),
                name: Some(res.kind.clone()),
                package: None,
            },
            capability: None,
            value: reir::FactValue::True,
            evidence: vec![contract_evidence(
                &source_label,
                None,
                &format!("resource type: {}", res.kind),
            )],
            confidence: reir::Confidence {
                level: reir::ConfidenceLevel::Declared,
                source: Some("rssi-contract".to_owned()),
            },
            acquisition_mode: reir::AcquisitionMode::NormalizedInterface,
            precision: reir::Precision::Exact,
            unknown_reason: None,
        });
    }

    for n in &bom.native_boundaries {
        facts.push(reir::Fact {
            schema: "reir.fact.v0.2".to_owned(),
            id: format!("contract-native-{}", sanitize(&n.function)),
            kind: reir::FactKind::NativeBoundary,
            role: Some(reir::FactRole::Required),
            subject: reir::Subject {
                kind: reir::SubjectKind::CodeFunction,
                id: n.function.clone(),
                name: Some(n.function.clone()),
                package: None,
            },
            capability: None,
            value: reir::FactValue::True,
            evidence: vec![contract_evidence(
                &source_label,
                Some(&n.function),
                "native boundary",
            )],
            confidence: reir::Confidence {
                level: reir::ConfidenceLevel::Declared,
                source: Some("rssi-contract".to_owned()),
            },
            acquisition_mode: reir::AcquisitionMode::NormalizedInterface,
            precision: reir::Precision::Exact,
            unknown_reason: None,
        });
    }

    for c in &bom.capabilities {
        facts.push(reir::Fact {
            schema: "reir.fact.v0.2".to_owned(),
            id: format!("contract-cap-{}", sanitize(&c.name)),
            kind: reir::FactKind::Capability,
            role: Some(reir::FactRole::Required),
            subject: reir::Subject {
                kind: reir::SubjectKind::Capability,
                id: c.name.clone(),
                name: Some(c.name.clone()),
                package: None,
            },
            capability: Some(reir::Capability {
                category: reir::CapabilityCategory::Extension("custom".to_owned()),
                provider: None,
                service: None,
                action: Some(c.name.clone()),
                resource: None,
                constraints: Default::default(),
            }),
            value: reir::FactValue::True,
            evidence: vec![contract_evidence(
                &source_label,
                None,
                &format!("capability: {}", c.name),
            )],
            confidence: reir::Confidence {
                level: reir::ConfidenceLevel::Declared,
                source: Some("rssi-contract".to_owned()),
            },
            acquisition_mode: reir::AcquisitionMode::NormalizedInterface,
            precision: reir::Precision::Exact,
            unknown_reason: None,
        });
    }

    let mut bundle = reir::Bundle::new();
    bundle.facts = facts;
    bundle
}

fn contract_evidence(file: &str, symbol: Option<&str>, reason: &str) -> reir::Evidence {
    reir::Evidence {
        kind: reir::EvidenceKind::InterfaceSpan,
        file: Some(file.to_owned()),
        line: None,
        column: None,
        length: None,
        symbol: symbol.map(str::to_owned),
        reason: Some(reason.to_owned()),
        json_pointer: None,
        resource: None,
        provider: None,
        value: None,
        event_id: None,
        time: None,
        source: None,
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Produce a BehaviorBom from a REIR bundle (the "BBOM as REIR slice" mode).
/// This derives the human-facing BBOM view from structured REIR facts rather than
/// re-analyzing source. Use this when REIR facts are already available.
pub fn behavior_bom_from_reir(bundle: &reir::Bundle) -> BehaviorBom {
    let mut mutations = Vec::new();
    let mut retentions = Vec::new();
    let mut resources = Vec::new();
    let mut native_boundaries = Vec::new();
    let mut capabilities = Vec::new();
    let mut unknown_functions = Vec::new();

    for fact in &bundle.facts {
        let function = fact.subject.name.clone().unwrap_or_else(|| {
            fact.subject
                .id
                .rsplit("::")
                .next()
                .unwrap_or(&fact.subject.id)
                .to_owned()
        });

        match &fact.kind {
            reir::FactKind::Mutation => {
                mutations.push(BomMutation {
                    target: fact
                        .evidence
                        .first()
                        .and_then(|e| e.symbol.clone())
                        .unwrap_or_else(|| "unknown".to_owned()),
                    function: function.clone(),
                    kind: BomMutationKind::MutParameter,
                });
            }
            reir::FactKind::Retention => {
                retentions.push(BomRetention {
                    parameter: fact
                        .evidence
                        .first()
                        .and_then(|e| e.symbol.clone())
                        .unwrap_or_else(|| "unknown".to_owned()),
                    function: function.clone(),
                    kind: BomRetentionKind::Retains,
                });
            }
            reir::FactKind::Resource => {
                resources.push(BomResource {
                    kind: fact
                        .capability
                        .as_ref()
                        .and_then(|c| c.service.clone())
                        .unwrap_or_else(|| "resource".to_owned()),
                    function: function.clone(),
                });
            }
            reir::FactKind::NativeBoundary => {
                native_boundaries.push(BomNativeBoundary {
                    call: fact
                        .evidence
                        .first()
                        .and_then(|e| e.symbol.clone())
                        .unwrap_or_else(|| "native".to_owned()),
                    function: function.clone(),
                    kind: BomBoundaryKind::Native,
                });
            }
            reir::FactKind::UnsafeBoundary => {
                native_boundaries.push(BomNativeBoundary {
                    call: fact
                        .evidence
                        .first()
                        .and_then(|e| e.symbol.clone())
                        .unwrap_or_else(|| "unsafe".to_owned()),
                    function: function.clone(),
                    kind: BomBoundaryKind::Unsafe,
                });
            }
            reir::FactKind::Capability => {
                capabilities.push(BomCapability {
                    name: fact
                        .capability
                        .as_ref()
                        .and_then(|c| c.action.clone())
                        .unwrap_or_else(|| "unknown".to_owned()),
                    function: function.clone(),
                });
                if fact.value == reir::FactValue::Unknown {
                    unknown_functions.push(BomUnknownFunction {
                        name: function.clone(),
                        line: fact.evidence.first().and_then(|e| e.line).unwrap_or(0),
                        unresolved_calls: vec![],
                    });
                }
            }
            _ => {}
        }
    }

    let total_functions = bundle.subjects.len();
    let unknown_count = unknown_functions.len();
    let review_count =
        mutations.len() + retentions.len() + resources.len() + native_boundaries.len();
    let unknown_ratio = if total_functions > 0 {
        unknown_count as f64 / total_functions as f64
    } else {
        0.0
    };
    let review_ratio = if total_functions > 0 {
        review_count as f64 / total_functions as f64
    } else {
        0.0
    };

    BehaviorBom {
        version: "0.2.0",
        files: bundle
            .producers
            .iter()
            .filter_map(|p| p.source.clone())
            .collect(),
        summary: BomSummary {
            total_functions,
            total_lines: 0,
            review_required_functions: review_count,
            foldable_functions: total_functions.saturating_sub(review_count + unknown_count),
            unknown_functions: unknown_count,
            unknown_ratio,
            review_ratio,
        },
        mutations,
        retentions,
        resources,
        native_boundaries,
        capabilities,
        unknown: BomUnknown {
            functions: unknown_functions,
            total_lines: 0,
        },
    }
}
