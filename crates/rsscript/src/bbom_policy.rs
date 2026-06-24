//! Capability-delta diff and merge-policy engine for the Behavior Bill of
//! Materials (BBOM).
//!
//! Answers "what new behaviors does this change introduce?" and gates
//! auto-merge based on a declarative [`MergePolicy`].

use serde::Serialize;

use crate::bbom::{
    BehaviorBom, BomCapability, BomMutation, BomNativeBoundary, BomResource, BomRetention,
};

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
