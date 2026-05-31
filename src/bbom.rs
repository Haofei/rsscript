//! Behavior Bill of Materials (BBOM)
//!
//! Produces a structured manifest of what a program *does* — not what it depends on.
//! This is the behavioral counterpart to SBOM: instead of listing dependencies,
//! it lists capabilities, mutations, retentions, resource access, native boundaries,
//! and unknown surface area.

use serde::Serialize;

use crate::review::{ReviewMapClassification, review_map_sources_with_interfaces};
use crate::syntax::ast::{DataEffect, EffectDecl, FunctionDecl, Item, Program, TypeRef};
use crate::syntax::parse_source;

#[derive(Debug, Clone, Serialize)]
pub struct BehaviorBom {
    pub version: &'static str,
    pub files: Vec<String>,
    pub summary: BomSummary,
    pub mutations: Vec<BomMutation>,
    pub retentions: Vec<BomRetention>,
    pub resources: Vec<BomResource>,
    pub native_boundaries: Vec<BomNativeBoundary>,
    pub capabilities: Vec<BomCapability>,
    pub unknown: BomUnknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomSummary {
    pub total_functions: usize,
    pub total_lines: usize,
    pub review_required_functions: usize,
    pub foldable_functions: usize,
    pub unknown_functions: usize,
    pub unknown_ratio: f64,
    pub review_ratio: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomMutation {
    pub target: String,
    pub function: String,
    pub kind: BomMutationKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BomMutationKind {
    MutParameter,
    TakeParameter,
    HandleFieldWrite,
    ManagedStateWrite,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomRetention {
    pub parameter: String,
    pub function: String,
    pub kind: BomRetentionKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BomRetentionKind {
    Retains,
    ManagedClosureCapture,
    SpawnCapture,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomResource {
    pub kind: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomNativeBoundary {
    pub call: String,
    pub function: String,
    pub kind: BomBoundaryKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BomBoundaryKind {
    Native,
    Unsafe,
    Parallel,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomCapability {
    pub name: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomUnknown {
    pub functions: Vec<BomUnknownFunction>,
    pub total_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomUnknownFunction {
    pub name: String,
    pub line: usize,
    pub unresolved_calls: Vec<String>,
}

/// Generate a Behavior BOM from source files.
pub fn behavior_bom_sources(sources: Vec<(&str, &str)>) -> BehaviorBom {
    behavior_bom_sources_with_interfaces(sources, &[])
}

/// Generate a Behavior BOM from source files with interface declarations.
pub fn behavior_bom_sources_with_interfaces(
    sources: Vec<(&str, &str)>,
    interfaces: &[(&str, &str)],
) -> BehaviorBom {
    let review_map = review_map_sources_with_interfaces(
        sources.iter().map(|(f, s)| (*f, *s)).collect(),
        interfaces,
    );

    let programs: Vec<(&str, Program)> = sources
        .iter()
        .map(|(file, source)| (*file, parse_source(file, source)))
        .collect();

    let mut mutations = Vec::new();
    let mut retentions = Vec::new();
    let mut resources = Vec::new();
    let mut native_boundaries = Vec::new();
    let mut capabilities = Vec::new();

    for (_file, program) in &programs {
        for item in &program.items {
            if let Item::Function(function) = item {
                collect_function_behaviors(
                    function,
                    &mut mutations,
                    &mut retentions,
                    &mut resources,
                    &mut native_boundaries,
                    &mut capabilities,
                );
            }
        }
    }

    let mut unknown_functions = Vec::new();
    let mut unknown_lines = 0usize;
    for file in &review_map.files {
        for region in &file.regions {
            if region.classification == ReviewMapClassification::Unknown {
                let unresolved: Vec<String> = region
                    .reasons
                    .iter()
                    .filter(|r| r.starts_with("unresolved call"))
                    .map(|r| r.trim_start_matches("unresolved call(s): ").to_string())
                    .collect();
                unknown_functions.push(BomUnknownFunction {
                    name: region.function.clone(),
                    line: region.line,
                    unresolved_calls: unresolved,
                });
                unknown_lines += region.line_count;
            }
        }
    }

    let total_functions = review_map.summary.total_functions;
    let total_lines = review_map.summary.total_lines;

    BehaviorBom {
        version: "0.1",
        files: sources.iter().map(|(f, _)| f.to_string()).collect(),
        summary: BomSummary {
            total_functions,
            total_lines,
            review_required_functions: review_map.summary.review_required.functions,
            foldable_functions: review_map.summary.foldable.functions,
            unknown_functions: review_map.summary.unknown.functions,
            unknown_ratio: if total_lines > 0 {
                unknown_lines as f64 / total_lines as f64
            } else {
                0.0
            },
            review_ratio: if total_lines > 0 {
                review_map.summary.must_review_lines as f64 / total_lines as f64
            } else {
                0.0
            },
        },
        mutations,
        retentions,
        resources,
        native_boundaries,
        capabilities,
        unknown: BomUnknown {
            functions: unknown_functions,
            total_lines: unknown_lines,
        },
    }
}

fn collect_function_behaviors(
    function: &FunctionDecl,
    mutations: &mut Vec<BomMutation>,
    retentions: &mut Vec<BomRetention>,
    resources: &mut Vec<BomResource>,
    native_boundaries: &mut Vec<BomNativeBoundary>,
    capabilities: &mut Vec<BomCapability>,
) {
    let fname = &function.name;

    // Mutations from parameters
    for param in &function.params {
        match param.effect {
            Some(DataEffect::Mut) => mutations.push(BomMutation {
                target: param.name.clone(),
                function: fname.clone(),
                kind: BomMutationKind::MutParameter,
            }),
            Some(DataEffect::Take) => mutations.push(BomMutation {
                target: param.name.clone(),
                function: fname.clone(),
                kind: BomMutationKind::TakeParameter,
            }),
            _ => {}
        }
    }

    // Effects declarations
    for effect in &function.effects {
        match effect {
            EffectDecl::Retains(param) => retentions.push(BomRetention {
                parameter: param.clone(),
                function: fname.clone(),
                kind: BomRetentionKind::Retains,
            }),
            EffectDecl::Name(name) => match name.as_str() {
                "native" => native_boundaries.push(BomNativeBoundary {
                    call: fname.clone(),
                    function: fname.clone(),
                    kind: BomBoundaryKind::Native,
                }),
                "unsafe" => native_boundaries.push(BomNativeBoundary {
                    call: fname.clone(),
                    function: fname.clone(),
                    kind: BomBoundaryKind::Unsafe,
                }),
                "parallel" => native_boundaries.push(BomNativeBoundary {
                    call: fname.clone(),
                    function: fname.clone(),
                    kind: BomBoundaryKind::Parallel,
                }),
                other => capabilities.push(BomCapability {
                    name: other.to_string(),
                    function: fname.clone(),
                }),
            },
        }
    }

    // Resource usage from parameter types
    for param in &function.params {
        if type_ref_is_resource_like(&param.ty) {
            resources.push(BomResource {
                kind: type_ref_name_string(&param.ty),
                function: fname.clone(),
            });
        }
    }

    // Async boundary
    if function.is_async {
        capabilities.push(BomCapability {
            name: "async".to_string(),
            function: fname.clone(),
        });
    }
}

fn type_ref_is_resource_like(ty: &TypeRef) -> bool {
    matches!(
        ty.name.as_str(),
        "File"
            | "Directory"
            | "DbConnection"
            | "HttpClient"
            | "ResourcePool"
            | "TcpStream"
            | "UdpSocket"
            | "Process"
    )
}

fn type_ref_name_string(ty: &TypeRef) -> String {
    ty.name.clone()
}

/// Format BBOM as human-readable text.
pub fn format_bbom_human(bom: &BehaviorBom) -> String {
    let mut out = String::new();

    out.push_str("╔══════════════════════════════════════╗\n");
    out.push_str("║   BEHAVIOR BILL OF MATERIALS (BBOM) ║\n");
    out.push_str("╚══════════════════════════════════════╝\n\n");

    out.push_str(&format!("files: {}\n", bom.files.join(", ")));
    out.push_str(&format!(
        "functions: {} total ({} review-required, {} foldable, {} unknown)\n",
        bom.summary.total_functions,
        bom.summary.review_required_functions,
        bom.summary.foldable_functions,
        bom.summary.unknown_functions
    ));
    out.push_str(&format!(
        "unknown ratio: {:.1}%\n",
        bom.summary.unknown_ratio * 100.0
    ));
    out.push_str(&format!(
        "review ratio: {:.1}%\n\n",
        bom.summary.review_ratio * 100.0
    ));

    if !bom.mutations.is_empty() {
        out.push_str("mutations:\n");
        for m in &bom.mutations {
            out.push_str(&format!(
                "  {:?} {} (in {})\n",
                m.kind, m.target, m.function
            ));
        }
        out.push('\n');
    }

    if !bom.retentions.is_empty() {
        out.push_str("retentions:\n");
        for r in &bom.retentions {
            out.push_str(&format!(
                "  {:?} {} (in {})\n",
                r.kind, r.parameter, r.function
            ));
        }
        out.push('\n');
    }

    if !bom.resources.is_empty() {
        out.push_str("resources:\n");
        for r in &bom.resources {
            out.push_str(&format!("  {} (in {})\n", r.kind, r.function));
        }
        out.push('\n');
    }

    if !bom.native_boundaries.is_empty() {
        out.push_str("native boundaries:\n");
        for b in &bom.native_boundaries {
            out.push_str(&format!("  {:?} {} (in {})\n", b.kind, b.call, b.function));
        }
        out.push('\n');
    }

    if !bom.capabilities.is_empty() {
        out.push_str("capabilities:\n");
        for c in &bom.capabilities {
            out.push_str(&format!("  {} (in {})\n", c.name, c.function));
        }
        out.push('\n');
    }

    if !bom.unknown.functions.is_empty() {
        out.push_str("unknown:\n");
        out.push_str(&format!(
            "  {} functions, {} lines\n",
            bom.unknown.functions.len(),
            bom.unknown.total_lines
        ));
        for f in &bom.unknown.functions {
            out.push_str(&format!("  {} (line {})", f.name, f.line));
            if !f.unresolved_calls.is_empty() {
                out.push_str(&format!(" — unresolved: {}", f.unresolved_calls.join(", ")));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    out
}

/// Format BBOM as JSON.
pub fn format_bbom_json(bom: &BehaviorBom) -> String {
    serde_json::to_string_pretty(bom).expect("BBOM JSON serialization should not fail")
}

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
            id: format!("contract-mut-{}-{}", sanitize(&m.function), sanitize(&m.target)),
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
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' })
        .collect()
}

// ─── Capability Delta ───────────────────────────────────────────────────────

/// A capability delta between two versions of a program.
/// This answers: "What new behaviors does this change introduce?"
#[derive(Debug, Clone, Serialize)]
pub struct CapabilityDelta {
    pub added_mutations: Vec<BomMutation>,
    pub removed_mutations: Vec<BomMutation>,
    pub added_retentions: Vec<BomRetention>,
    pub removed_retentions: Vec<BomRetention>,
    pub added_resources: Vec<BomResource>,
    pub removed_resources: Vec<BomResource>,
    pub added_native_boundaries: Vec<BomNativeBoundary>,
    pub removed_native_boundaries: Vec<BomNativeBoundary>,
    pub added_capabilities: Vec<BomCapability>,
    pub removed_capabilities: Vec<BomCapability>,
    pub unknown_delta: i64,
    pub verdict: CapabilityVerdict,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilityVerdict {
    pub safe_to_auto_merge: bool,
    pub reasons: Vec<String>,
}

/// Compute the capability delta between old and new BOM.
pub fn capability_delta(old: &BehaviorBom, new: &BehaviorBom) -> CapabilityDelta {
    let added_mutations = diff_items(&old.mutations, &new.mutations, mutation_key);
    let removed_mutations = diff_items(&new.mutations, &old.mutations, mutation_key);
    let added_retentions = diff_items(&old.retentions, &new.retentions, retention_key);
    let removed_retentions = diff_items(&new.retentions, &old.retentions, retention_key);
    let added_resources = diff_items(&old.resources, &new.resources, resource_key);
    let removed_resources = diff_items(&new.resources, &old.resources, resource_key);
    let added_native = diff_items(&old.native_boundaries, &new.native_boundaries, native_key);
    let removed_native = diff_items(&new.native_boundaries, &old.native_boundaries, native_key);
    let added_capabilities = diff_items(&old.capabilities, &new.capabilities, capability_key);
    let removed_capabilities = diff_items(&new.capabilities, &old.capabilities, capability_key);
    let unknown_delta = new.summary.unknown_functions as i64 - old.summary.unknown_functions as i64;

    let mut reasons = Vec::new();
    if !added_native.is_empty() {
        reasons.push(format!("+{} native boundary(ies)", added_native.len()));
    }
    if !added_retentions.is_empty() {
        reasons.push(format!("+{} retention(s)", added_retentions.len()));
    }
    if !added_resources.is_empty() {
        reasons.push(format!("+{} resource(s)", added_resources.len()));
    }
    if unknown_delta > 0 {
        reasons.push(format!("+{unknown_delta} unknown function(s)"));
    }
    if !added_capabilities.is_empty() {
        reasons.push(format!("+{} capability(ies)", added_capabilities.len()));
    }

    let safe_to_auto_merge = added_native.is_empty()
        && added_resources.is_empty()
        && unknown_delta <= 0
        && added_capabilities.is_empty();

    CapabilityDelta {
        added_mutations,
        removed_mutations,
        added_retentions,
        removed_retentions,
        added_resources,
        removed_resources,
        added_native_boundaries: added_native,
        removed_native_boundaries: removed_native,
        added_capabilities,
        removed_capabilities,
        unknown_delta,
        verdict: CapabilityVerdict {
            safe_to_auto_merge,
            reasons,
        },
    }
}

/// Format capability delta as human-readable text.
pub fn format_capability_delta_human(delta: &CapabilityDelta) -> String {
    let mut out = String::new();

    out.push_str("╔══════════════════════════════════════╗\n");
    out.push_str("║       CAPABILITY DELTA               ║\n");
    out.push_str("╚══════════════════════════════════════╝\n\n");

    if delta.verdict.safe_to_auto_merge {
        out.push_str("verdict: ✅ SAFE TO AUTO-MERGE\n\n");
    } else {
        out.push_str("verdict: ⚠️  REQUIRES HUMAN REVIEW\n");
        for reason in &delta.verdict.reasons {
            out.push_str(&format!("  • {reason}\n"));
        }
        out.push('\n');
    }

    if !delta.added_mutations.is_empty() {
        out.push_str("+ mutations:\n");
        for m in &delta.added_mutations {
            out.push_str(&format!(
                "    + {:?} {} (in {})\n",
                m.kind, m.target, m.function
            ));
        }
    }
    if !delta.removed_mutations.is_empty() {
        out.push_str("- mutations:\n");
        for m in &delta.removed_mutations {
            out.push_str(&format!(
                "    - {:?} {} (in {})\n",
                m.kind, m.target, m.function
            ));
        }
    }
    if !delta.added_retentions.is_empty() {
        out.push_str("+ retentions:\n");
        for r in &delta.added_retentions {
            out.push_str(&format!(
                "    + {:?} {} (in {})\n",
                r.kind, r.parameter, r.function
            ));
        }
    }
    if !delta.removed_retentions.is_empty() {
        out.push_str("- retentions:\n");
        for r in &delta.removed_retentions {
            out.push_str(&format!(
                "    - {:?} {} (in {})\n",
                r.kind, r.parameter, r.function
            ));
        }
    }
    if !delta.added_resources.is_empty() {
        out.push_str("+ resources:\n");
        for r in &delta.added_resources {
            out.push_str(&format!("    + {} (in {})\n", r.kind, r.function));
        }
    }
    if !delta.removed_resources.is_empty() {
        out.push_str("- resources:\n");
        for r in &delta.removed_resources {
            out.push_str(&format!("    - {} (in {})\n", r.kind, r.function));
        }
    }
    if !delta.added_native_boundaries.is_empty() {
        out.push_str("+ native boundaries:\n");
        for b in &delta.added_native_boundaries {
            out.push_str(&format!(
                "    + {:?} {} (in {})\n",
                b.kind, b.call, b.function
            ));
        }
    }
    if !delta.removed_native_boundaries.is_empty() {
        out.push_str("- native boundaries:\n");
        for b in &delta.removed_native_boundaries {
            out.push_str(&format!(
                "    - {:?} {} (in {})\n",
                b.kind, b.call, b.function
            ));
        }
    }
    if !delta.added_capabilities.is_empty() {
        out.push_str("+ capabilities:\n");
        for c in &delta.added_capabilities {
            out.push_str(&format!("    + {} (in {})\n", c.name, c.function));
        }
    }
    if !delta.removed_capabilities.is_empty() {
        out.push_str("- capabilities:\n");
        for c in &delta.removed_capabilities {
            out.push_str(&format!("    - {} (in {})\n", c.name, c.function));
        }
    }
    if delta.unknown_delta != 0 {
        out.push_str(&format!("unknown delta: {:+}\n", delta.unknown_delta));
    }

    if delta.added_mutations.is_empty()
        && delta.removed_mutations.is_empty()
        && delta.added_retentions.is_empty()
        && delta.removed_retentions.is_empty()
        && delta.added_resources.is_empty()
        && delta.removed_resources.is_empty()
        && delta.added_native_boundaries.is_empty()
        && delta.removed_native_boundaries.is_empty()
        && delta.added_capabilities.is_empty()
        && delta.removed_capabilities.is_empty()
        && delta.unknown_delta == 0
    {
        out.push_str("no behavioral changes detected\n");
    }

    out
}

/// Format capability delta as JSON.
pub fn format_capability_delta_json(delta: &CapabilityDelta) -> String {
    serde_json::to_string_pretty(delta)
        .expect("capability delta JSON serialization should not fail")
}

// ─── Merge Policy ───────────────────────────────────────────────────────────

/// A declarative merge policy that gates auto-merge based on BBOM/delta.
#[derive(Debug, Clone, Serialize)]
pub struct MergePolicy {
    pub max_unknown_ratio: f64,
    pub allow_new_native: bool,
    pub allow_new_resources: bool,
    pub allow_new_retentions: bool,
    pub allow_unknown_increase: bool,
    pub max_new_mutations: Option<usize>,
}

impl Default for MergePolicy {
    fn default() -> Self {
        Self {
            max_unknown_ratio: 0.02,
            allow_new_native: false,
            allow_new_resources: false,
            allow_new_retentions: true,
            allow_unknown_increase: false,
            max_new_mutations: None,
        }
    }
}

/// Result of evaluating a merge policy.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyResult {
    pub pass: bool,
    pub violations: Vec<String>,
}

/// Evaluate a merge policy against a capability delta and new BOM.
pub fn evaluate_policy(
    policy: &MergePolicy,
    delta: &CapabilityDelta,
    new_bom: &BehaviorBom,
) -> PolicyResult {
    let mut violations = Vec::new();

    if new_bom.summary.unknown_ratio > policy.max_unknown_ratio {
        violations.push(format!(
            "unknown_ratio {:.1}% exceeds maximum {:.1}%",
            new_bom.summary.unknown_ratio * 100.0,
            policy.max_unknown_ratio * 100.0
        ));
    }
    if !policy.allow_new_native && !delta.added_native_boundaries.is_empty() {
        violations.push(format!(
            "{} new native boundary(ies) not allowed",
            delta.added_native_boundaries.len()
        ));
    }
    if !policy.allow_new_resources && !delta.added_resources.is_empty() {
        violations.push(format!(
            "{} new resource(s) not allowed",
            delta.added_resources.len()
        ));
    }
    if !policy.allow_new_retentions && !delta.added_retentions.is_empty() {
        violations.push(format!(
            "{} new retention(s) not allowed",
            delta.added_retentions.len()
        ));
    }
    if !policy.allow_unknown_increase && delta.unknown_delta > 0 {
        violations.push(format!(
            "unknown function count increased by {}",
            delta.unknown_delta
        ));
    }
    if let Some(max) = policy.max_new_mutations {
        if delta.added_mutations.len() > max {
            violations.push(format!(
                "{} new mutation(s) exceeds maximum of {max}",
                delta.added_mutations.len()
            ));
        }
    }

    PolicyResult {
        pass: violations.is_empty(),
        violations,
    }
}

/// Format policy result as human-readable text.
pub fn format_policy_result_human(result: &PolicyResult) -> String {
    let mut out = String::new();
    if result.pass {
        out.push_str("policy: ✅ PASS — auto-merge allowed\n");
    } else {
        out.push_str("policy: ❌ FAIL — human review required\n");
        for violation in &result.violations {
            out.push_str(&format!("  • {violation}\n"));
        }
    }
    out
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn diff_items<T: Clone>(old: &[T], new: &[T], key: fn(&T) -> String) -> Vec<T> {
    let old_keys: std::collections::BTreeSet<String> = old.iter().map(|item| key(item)).collect();
    new.iter()
        .filter(|item| !old_keys.contains(&key(item)))
        .cloned()
        .collect()
}

fn mutation_key(m: &BomMutation) -> String {
    format!("{}.{:?}", m.function, m.kind)
}

fn retention_key(r: &BomRetention) -> String {
    format!("{}.{}", r.function, r.parameter)
}

fn resource_key(r: &BomResource) -> String {
    format!("{}.{}", r.function, r.kind)
}

fn native_key(b: &BomNativeBoundary) -> String {
    format!("{}.{:?}", b.function, b.kind)
}

fn capability_key(c: &BomCapability) -> String {
    format!("{}.{}", c.function, c.name)
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
