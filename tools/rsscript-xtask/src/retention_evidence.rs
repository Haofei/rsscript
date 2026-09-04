use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
struct BenchmarkEvidence {
    evidence_class: String,
    controlled: bool,
    cases: Vec<BenchmarkCase>,
}

#[derive(Deserialize)]
struct BenchmarkCase {
    case: String,
    status: String,
    speedup: f64,
    native_bails: u64,
    semantic_match: bool,
    controlled: bool,
    retention_threshold_met: bool,
}

pub(super) fn minimum_verified_gain_percent(
    bytes: &[u8],
    decision_basis: &str,
    evidence_cases: &[String],
) -> Result<f64, String> {
    let evidence: BenchmarkEvidence = serde_json::from_slice(bytes)
        .map_err(|error| format!("performance evidence is not benchmark JSON: {error}"))?;
    let (expected_class, expected_controlled) = match decision_basis {
        "controlled-performance" => ("controlled-canonical", true),
        "local-performance" => ("local-diagnostic", false),
        _ => return Err(format!("unsupported performance basis `{decision_basis}`")),
    };
    if evidence.evidence_class != expected_class || evidence.controlled != expected_controlled {
        return Err(format!(
            "performance evidence class `{}`/controlled={} does not match `{decision_basis}`",
            evidence.evidence_class, evidence.controlled
        ));
    }
    if evidence_cases.is_empty() {
        return Err("performance evidence declares no case mapping".to_string());
    }
    let mut requested = BTreeSet::new();
    let mut minimum_gain = f64::INFINITY;
    for name in evidence_cases {
        if !requested.insert(name) {
            return Err(format!("performance evidence case `{name}` is duplicated"));
        }
        let mut matches = evidence.cases.iter().filter(|case| case.case == *name);
        let case = matches
            .next()
            .ok_or_else(|| format!("performance evidence is missing case `{name}`"))?;
        if matches.next().is_some() {
            return Err(format!("performance evidence case `{name}` is duplicated"));
        }
        if case.status != "entered"
            || case.native_bails != 0
            || !case.semantic_match
            || !case.retention_threshold_met
            || case.controlled != expected_controlled
        {
            return Err(format!(
                "performance evidence case `{name}` is not a successful, matching, bail-free measurement"
            ));
        }
        if !case.speedup.is_finite() || case.speedup < 1.0 {
            return Err(format!(
                "performance evidence case `{name}` has invalid speedup {}",
                case.speedup
            ));
        }
        let gain = (case.speedup - 1.0) * 100.0;
        minimum_gain = minimum_gain.min(gain);
    }
    Ok(minimum_gain)
}

#[cfg(test)]
mod tests {
    use super::minimum_verified_gain_percent;

    fn evidence(class: &str, controlled: bool, threshold_met: bool) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "evidence_class": class,
            "controlled": controlled,
            "cases": [{
                "case": "scalar-loop",
                "status": "entered",
                "speedup": 1.25,
                "native_bails": 0,
                "semantic_match": true,
                "controlled": controlled,
                "retention_threshold_met": threshold_met
            }]
        }))
        .expect("serialize evidence")
    }

    #[test]
    fn derives_the_minimum_gain_from_the_named_case() {
        assert_eq!(
            minimum_verified_gain_percent(
                &evidence("local-diagnostic", false, true),
                "local-performance",
                &["scalar-loop".to_string()]
            ),
            Ok(25.0)
        );
    }

    #[test]
    fn rejects_class_and_threshold_drift() {
        assert!(
            minimum_verified_gain_percent(
                &evidence("controlled-canonical", true, true),
                "local-performance",
                &["scalar-loop".to_string()]
            )
            .is_err()
        );
        assert!(
            minimum_verified_gain_percent(
                &evidence("local-diagnostic", false, false),
                "local-performance",
                &["scalar-loop".to_string()]
            )
            .is_err()
        );
    }
}
