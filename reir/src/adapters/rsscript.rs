//! RSScript language and package producer adapter for REIR.
//! Converts RSScript compiler/package review output into REIR facts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::*;

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const EDGE_SCHEMA: &str = "reir.edge.v0.1";
const PRODUCER_VERSION: &str = "0.5.0";
const ADAPTER_VERSION: &str = "0.1";
const PRODUCER_SOURCE: &str = "compiler_contract";
const REVIEW_MAP_SOURCE: &str = "rsscript_review_map";
const PACKAGE_REVIEW_SOURCE: &str = "rsscript_package_review";
const REVIEW_REQUIRED_KIND: &str = "review_required";

/// Input from RSScript review-map (mirrors what the compiler produces).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptReviewMapInput {
    pub package_name: String,
    pub regions: Vec<RsScriptRegionInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptRegionInput {
    pub file: String,
    pub function_name: String,
    pub classification: RsScriptClassification,
    pub line: usize,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RsScriptClassification {
    Foldable,
    ReviewRequired,
    Unknown,
}

/// Input from RSScript package review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptPackageReviewInput {
    pub package_name: String,
    pub version: String,
    pub risk: RsScriptPackageRisk,
    pub public_apis: usize,
    pub mutating_apis: usize,
    pub retaining_apis: usize,
    pub resource_apis: usize,
    pub native_apis: usize,
    pub unsafe_apis: usize,
    pub unknown_apis: usize,
    pub native_boundaries: Vec<RsScriptNativeBoundary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RsScriptPackageRisk {
    Low,
    Elevated,
    High,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsScriptNativeBoundary {
    pub module_name: String,
    pub functions: Vec<String>,
    pub file: String,
    pub line: usize,
}

/// Convert RSScript review-map output into REIR facts.
pub fn review_map_to_facts(input: &RsScriptReviewMapInput) -> Vec<Fact> {
    input
        .regions
        .iter()
        .filter_map(|region| match region.classification {
            RsScriptClassification::Foldable => None,
            RsScriptClassification::Unknown => Some(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.review_map.{}.unknown",
                    normalized_id(&function_subject_id(
                        &input.package_name,
                        &region.function_name
                    ))
                ),
                kind: FactKind::Unknown,
                role: None,
                subject: function_subject(&input.package_name, &region.function_name),
                capability: None,
                value: FactValue::Unknown,
                confidence: confidence(ConfidenceLevel::Unknown, REVIEW_MAP_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![source_span(
                    &region.file,
                    region.line,
                    &region.function_name,
                    joined_reason(&region.reasons),
                    REVIEW_MAP_SOURCE,
                )],
                unknown_reason: joined_reason(&region.reasons),
            }),
            RsScriptClassification::ReviewRequired => {
                let kind = classify_review_required(&region.reasons);
                let kind_name: String = kind.clone().into();
                Some(Fact {
                    schema: FACT_SCHEMA.to_owned(),
                    id: format!(
                        "fact.review_map.{}.{}",
                        normalized_id(&function_subject_id(
                            &input.package_name,
                            &region.function_name
                        )),
                        normalized_id(&kind_name)
                    ),
                    kind,
                    role: None,
                    subject: function_subject(&input.package_name, &region.function_name),
                    capability: None,
                    value: FactValue::True,
                    confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
                    acquisition_mode: AcquisitionMode::CompilerContract,
                    precision: Precision::Exact,
                    evidence: vec![source_span(
                        &region.file,
                        region.line,
                        &region.function_name,
                        joined_reason(&region.reasons),
                        REVIEW_MAP_SOURCE,
                    )],
                    unknown_reason: None,
                })
            }
        })
        .collect()
}

/// Convert RSScript package review into REIR facts.
pub fn package_review_to_facts(input: &RsScriptPackageReviewInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    let package_subject = package_subject(&input.package_name, &input.version);
    let package_slug = normalized_id(&package_subject.id);
    let package_summary = package_review_summary(input);

    facts.push(Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package.{}.risk", package_slug),
        kind: FactKind::PackageRisk,
        role: None,
        subject: package_subject.clone(),
        capability: None,
        value: package_risk_value(&input.risk),
        confidence: confidence(package_risk_confidence(&input.risk), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Category,
        evidence: vec![package_metadata(
            &input.package_name,
            &input.version,
            Some(input.risk.as_str().to_owned()),
            Some(package_summary.clone()),
        )],
        unknown_reason: matches!(input.risk, RsScriptPackageRisk::Unknown)
            .then(|| "package risk could not be determined".to_owned()),
    });

    if input.native_apis > 0 {
        facts.push(capability_fact(
            format!("fact.package.{}.capability.runtime_native", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeNative,
            input.native_apis,
            &input.package_name,
            &input.version,
            package_summary.clone(),
        ));
    }

    if input.unsafe_apis > 0 {
        facts.push(capability_fact(
            format!("fact.package.{}.capability.runtime_unsafe", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeUnsafe,
            input.unsafe_apis,
            &input.package_name,
            &input.version,
            package_summary.clone(),
        ));
    }

    for boundary in &input.native_boundaries {
        let boundary_subject = native_boundary_subject(&input.package_name, &boundary.module_name);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.native_boundary.{}",
                normalized_id(&boundary_subject.id)
            ),
            kind: FactKind::NativeBoundary,
            role: None,
            subject: boundary_subject,
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![source_span(
                &boundary.file,
                boundary.line,
                &boundary.module_name,
                Some(native_boundary_reason(boundary)),
                PACKAGE_REVIEW_SOURCE,
            )],
            unknown_reason: None,
        });
    }

    facts
}

/// Convert RSScript native boundaries into REIR edges.
pub fn native_boundaries_to_edges(input: &RsScriptPackageReviewInput) -> Vec<Edge> {
    let mut edges = Vec::new();
    let package_subject = package_subject(&input.package_name, &input.version);

    for boundary in &input.native_boundaries {
        let to = native_boundary_subject(&input.package_name, &boundary.module_name);

        if boundary.functions.is_empty() {
            edges.push(native_edge(
                format!(
                    "edge.crosses_native.{}.{}",
                    normalized_id(&package_subject.id),
                    normalized_id(&to.id)
                ),
                package_subject.clone(),
                to.clone(),
                &boundary.file,
                boundary.line,
                &boundary.module_name,
            ));
            continue;
        }

        for function_name in &boundary.functions {
            let from = function_subject(&input.package_name, function_name);
            edges.push(native_edge(
                format!(
                    "edge.crosses_native.{}.{}",
                    normalized_id(&from.id),
                    normalized_id(&to.id)
                ),
                from,
                to.clone(),
                &boundary.file,
                boundary.line,
                function_name,
            ));
        }
    }

    edges
}

/// Build a complete REIR bundle from RSScript compiler output.
pub fn rsscript_to_bundle(
    review_map: &RsScriptReviewMapInput,
    package_review: &RsScriptPackageReviewInput,
) -> Bundle {
    let producer = Producer {
        name: "rssc".to_string(),
        version: PRODUCER_VERSION.to_string(),
        adapter: Some("rsscript-language".to_string()),
        adapter_version: Some(ADAPTER_VERSION.to_string()),
        source: Some(PRODUCER_SOURCE.to_string()),
    };

    let mut bundle = Bundle::new();
    bundle.producers.push(producer);
    bundle.facts.extend(review_map_to_facts(review_map));
    bundle.facts.extend(package_review_to_facts(package_review));
    bundle
        .edges
        .extend(native_boundaries_to_edges(package_review));
    bundle
}

impl RsScriptPackageRisk {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Elevated => "elevated",
            Self::High => "high",
            Self::Unknown => "unknown",
        }
    }
}

fn classify_review_required(reasons: &[String]) -> FactKind {
    let lower_reasons = reasons
        .iter()
        .map(|reason| reason.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if lower_reasons.iter().any(|reason| reason.contains("native")) {
        FactKind::NativeBoundary
    } else if lower_reasons.iter().any(|reason| reason.contains("retain")) {
        FactKind::Retention
    } else if lower_reasons.iter().any(|reason| reason.contains("mut")) {
        FactKind::Mutation
    } else {
        FactKind::Extension(REVIEW_REQUIRED_KIND.to_owned())
    }
}

fn function_subject(package_name: &str, function_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeFunction,
        id: function_subject_id(package_name, function_name),
        name: Some(function_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn function_subject_id(package_name: &str, function_name: &str) -> String {
    format!("{package_name}::{function_name}")
}

fn package_subject(package_name: &str, version: &str) -> Subject {
    Subject {
        kind: SubjectKind::Package,
        id: format!("{package_name}@{version}"),
        name: Some(package_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn native_boundary_subject(package_name: &str, module_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::NativeBoundary,
        id: format!("{package_name}::native::{module_name}"),
        name: Some(module_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn package_risk_value(risk: &RsScriptPackageRisk) -> FactValue {
    match risk {
        RsScriptPackageRisk::Unknown => FactValue::Unknown,
        _ => FactValue::True,
    }
}

fn package_risk_confidence(risk: &RsScriptPackageRisk) -> ConfidenceLevel {
    match risk {
        RsScriptPackageRisk::Unknown => ConfidenceLevel::Unknown,
        _ => ConfidenceLevel::Authoritative,
    }
}

fn capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    count: usize,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    let mut constraints = HashMap::new();
    constraints.insert("count".to_owned(), count.to_string());

    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: None,
            action: None,
            resource: Some(subject.id.clone()),
            constraints,
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(count.to_string()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn native_edge(
    id: String,
    from: Subject,
    to: Subject,
    file: &str,
    line: usize,
    symbol: &str,
) -> Edge {
    Edge {
        schema: EDGE_SCHEMA.to_owned(),
        id,
        kind: EdgeKind::CrossesNative,
        from,
        to,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![source_span(file, line, symbol, None, PACKAGE_REVIEW_SOURCE)],
    }
}

fn confidence(level: ConfidenceLevel, source: &str) -> Confidence {
    Confidence {
        level,
        source: Some(source.to_owned()),
    }
}

fn source_span(
    file: &str,
    line: usize,
    symbol: &str,
    reason: Option<String>,
    source: &str,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::SourceSpan,
        file: Some(file.to_owned()),
        line: Some(line),
        column: None,
        length: None,
        symbol: Some(symbol.to_owned()),
        reason,
        json_pointer: None,
        resource: None,
        provider: None,
        value: None,
        event_id: None,
        time: None,
        source: Some(source.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_metadata(
    package_name: &str,
    version: &str,
    value: Option<String>,
    reason: Option<String>,
) -> Evidence {
    Evidence {
        kind: EvidenceKind::PackageMetadata,
        file: None,
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason,
        json_pointer: None,
        resource: Some(format!("{package_name}@{version}")),
        provider: Some("rsscript".to_owned()),
        value,
        event_id: None,
        time: None,
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn package_review_summary(input: &RsScriptPackageReviewInput) -> String {
    format!(
        "public_apis={}, mutating_apis={}, retaining_apis={}, resource_apis={}, native_apis={}, unsafe_apis={}, unknown_apis={}, native_boundaries={}",
        input.public_apis,
        input.mutating_apis,
        input.retaining_apis,
        input.resource_apis,
        input.native_apis,
        input.unsafe_apis,
        input.unknown_apis,
        input.native_boundaries.len()
    )
}

fn native_boundary_reason(boundary: &RsScriptNativeBoundary) -> String {
    if boundary.functions.is_empty() {
        format!("native boundary in module {}", boundary.module_name)
    } else {
        format!(
            "native boundary in module {} for functions {}",
            boundary.module_name,
            boundary.functions.join(", ")
        )
    }
}

fn joined_reason(reasons: &[String]) -> Option<String> {
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

fn normalized_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_review_map() -> RsScriptReviewMapInput {
        RsScriptReviewMapInput {
            package_name: "demo_pkg".to_owned(),
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
        }
    }

    #[test]
    fn review_map_to_facts_skips_foldable_regions() {
        let facts = review_map_to_facts(&sample_review_map());

        assert_eq!(facts.len(), 2);
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

        assert_eq!(facts.len(), 4);
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
        assert_eq!(bundle.facts.len(), 6);
        assert_eq!(bundle.edges.len(), 1);
        assert_eq!(round_trip, bundle);
    }
}
