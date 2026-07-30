use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::{
    AcquisitionMode, Bundle, Confidence, ConfidenceLevel, Edge, Evidence, EvidenceKind, Fact,
    FactKind, FactValue, Precision, Producer, Subject, SubjectKind, slice_by_kind,
};

const FACT_SCHEMA: &str = "reir.fact.v0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AdapterLimits {
    pub max_operations: usize,
    pub max_facts: usize,
    pub max_string_bytes: usize,
}

impl AdapterLimits {
    pub const fn new(max_operations: usize, max_facts: usize, max_string_bytes: usize) -> Self {
        Self {
            max_operations,
            max_facts,
            max_string_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProducerProvenance {
    pub name: &'static str,
    pub version: &'static str,
    pub adapter: &'static str,
    pub adapter_version: &'static str,
    pub source: &'static str,
}

impl ProducerProvenance {
    fn into_producer(self) -> Result<Producer, AdapterBuildError> {
        for (field, value) in [
            ("name", self.name),
            ("version", self.version),
            ("adapter", self.adapter),
            ("adapter_version", self.adapter_version),
            ("source", self.source),
        ] {
            if value.trim().is_empty() {
                return Err(AdapterBuildError::MissingProvenance(field));
            }
        }
        Ok(Producer {
            name: self.name.to_owned(),
            version: self.version.to_owned(),
            adapter: Some(self.adapter.to_owned()),
            adapter_version: Some(self.adapter_version.to_owned()),
            source: Some(self.source.to_owned()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AdapterBuildError {
    OperationBudgetExceeded { limit: usize },
    FactBudgetExceeded { limit: usize },
    StringBudgetExceeded { limit: usize },
    CounterOverflow(&'static str),
    MissingProvenance(&'static str),
}

impl fmt::Display for AdapterBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OperationBudgetExceeded { limit } => {
                write!(formatter, "adapter operation budget exceeds limit {limit}")
            }
            Self::FactBudgetExceeded { limit } => {
                write!(formatter, "adapter fact budget exceeds limit {limit}")
            }
            Self::StringBudgetExceeded { limit } => {
                write!(
                    formatter,
                    "adapter string budget exceeds limit {limit} bytes"
                )
            }
            Self::CounterOverflow(counter) => {
                write!(formatter, "adapter {counter} counter overflow")
            }
            Self::MissingProvenance(field) => {
                write!(
                    formatter,
                    "adapter producer provenance is missing `{field}`"
                )
            }
        }
    }
}

pub(super) struct BoundedEvidenceBuilder {
    limits: AdapterLimits,
    operations: usize,
    string_bytes: usize,
    facts: Vec<Fact>,
    edges: Vec<Edge>,
}

impl BoundedEvidenceBuilder {
    pub fn new(limits: AdapterLimits) -> Self {
        Self {
            limits,
            operations: 0,
            string_bytes: 0,
            facts: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn record_operation(&mut self) -> Result<(), AdapterBuildError> {
        self.record_operations(1)
    }

    pub fn record_operations(&mut self, count: usize) -> Result<(), AdapterBuildError> {
        self.operations = self
            .operations
            .checked_add(count)
            .ok_or(AdapterBuildError::CounterOverflow("operation"))?;
        if self.operations > self.limits.max_operations {
            return Err(AdapterBuildError::OperationBudgetExceeded {
                limit: self.limits.max_operations,
            });
        }
        Ok(())
    }

    pub fn ensure_fact_capacity(&self, additional: usize) -> Result<(), AdapterBuildError> {
        let total = self
            .facts
            .len()
            .checked_add(additional)
            .ok_or(AdapterBuildError::CounterOverflow("fact"))?;
        if total > self.limits.max_facts {
            return Err(AdapterBuildError::FactBudgetExceeded {
                limit: self.limits.max_facts,
            });
        }
        Ok(())
    }

    pub fn push_fact(&mut self, fact: Fact) -> Result<(), AdapterBuildError> {
        self.ensure_fact_capacity(1)?;
        self.record_operation()?;
        self.charge_strings(&fact)?;
        self.facts.push(fact);
        Ok(())
    }

    pub fn extend_facts(
        &mut self,
        facts: impl IntoIterator<Item = Fact>,
    ) -> Result<(), AdapterBuildError> {
        for fact in facts {
            self.push_fact(fact)?;
        }
        Ok(())
    }

    pub fn extend_edges(
        &mut self,
        edges: impl IntoIterator<Item = Edge>,
    ) -> Result<(), AdapterBuildError> {
        for edge in edges {
            self.record_operation()?;
            self.charge_strings(&edge)?;
            self.edges.push(edge);
        }
        Ok(())
    }

    pub fn push_unknown_coverage(
        &mut self,
        coverage: UnknownCoverage<'_>,
    ) -> Result<(), AdapterBuildError> {
        self.push_fact(coverage.into_fact())
    }

    pub fn finish(self, producer: ProducerProvenance) -> Result<Bundle, AdapterBuildError> {
        self.finish_with_subjects(producer, SubjectIndex::Canonical)
    }

    pub fn finish_preserving_fact_subjects(
        self,
        producer: ProducerProvenance,
    ) -> Result<Bundle, AdapterBuildError> {
        self.finish_with_subjects(producer, SubjectIndex::FactOrder)
    }

    fn finish_with_subjects(
        self,
        producer: ProducerProvenance,
        subject_index: SubjectIndex,
    ) -> Result<Bundle, AdapterBuildError> {
        let mut bundle = Bundle::new();
        bundle.producers.push(producer.into_producer()?);
        bundle.facts = self.facts;
        bundle.edges = self.edges;
        match subject_index {
            SubjectIndex::Canonical => index_bundle_subjects(&mut bundle),
            SubjectIndex::FactOrder => {
                bundle.subjects = bundle
                    .facts
                    .iter()
                    .map(|fact| fact.subject.clone())
                    .collect();
            }
        }
        bundle.slices = slice_by_kind(&bundle);
        Ok(bundle)
    }

    fn charge_strings<T: Serialize>(&mut self, value: &T) -> Result<(), AdapterBuildError> {
        let value = serde_json::to_value(value)
            .map_err(|_| AdapterBuildError::CounterOverflow("serialization"))?;
        let additional = string_bytes(&value)?;
        self.string_bytes = self
            .string_bytes
            .checked_add(additional)
            .ok_or(AdapterBuildError::CounterOverflow("string byte"))?;
        if self.string_bytes > self.limits.max_string_bytes {
            return Err(AdapterBuildError::StringBudgetExceeded {
                limit: self.limits.max_string_bytes,
            });
        }
        Ok(())
    }
}

enum SubjectIndex {
    Canonical,
    FactOrder,
}

pub(super) struct UnknownCoverage<'a> {
    pub id: String,
    pub subject_kind: SubjectKind,
    pub subject_id: String,
    pub subject_name: String,
    pub package: &'a str,
    pub reason: String,
    pub source: &'a str,
    pub acquisition_mode: AcquisitionMode,
    pub evidence_kind: EvidenceKind,
    pub evidence_file: &'a str,
    pub evidence_pointer: Option<String>,
    pub evidence_value: &'a str,
}

impl UnknownCoverage<'_> {
    pub fn into_fact(self) -> Fact {
        Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: self.id,
            kind: FactKind::Diagnostic,
            role: None,
            subject: Subject {
                kind: self.subject_kind,
                id: self.subject_id,
                name: Some(self.subject_name.clone()),
                package: Some(self.package.to_owned()),
            },
            capability: None,
            value: FactValue::Unknown,
            confidence: Confidence {
                level: ConfidenceLevel::Authoritative,
                source: Some(self.source.to_owned()),
            },
            acquisition_mode: self.acquisition_mode,
            precision: Precision::Exact,
            evidence: vec![Evidence {
                kind: self.evidence_kind,
                file: Some(self.evidence_file.to_owned()),
                line: None,
                column: None,
                length: None,
                symbol: Some(self.subject_name.clone()),
                reason: Some(self.reason.clone()),
                json_pointer: self.evidence_pointer,
                resource: Some(self.subject_name),
                provider: Some(self.package.to_owned()),
                value: Some(self.evidence_value.to_owned()),
                event_id: None,
                time: None,
                source: Some(self.source.to_owned()),
                event_name: None,
                principal: None,
                account: None,
                policy_arn: None,
                statement_index: None,
                action: None,
            }],
            unknown_reason: Some(self.reason),
        }
    }
}

fn string_bytes(value: &Value) -> Result<usize, AdapterBuildError> {
    let mut total = 0_usize;
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::String(value) => {
                total = total
                    .checked_add(value.len())
                    .ok_or(AdapterBuildError::CounterOverflow("string byte"))?;
            }
            Value::Array(values) => stack.extend(values),
            Value::Object(values) => {
                for (key, value) in values {
                    total = total
                        .checked_add(key.len())
                        .ok_or(AdapterBuildError::CounterOverflow("string byte"))?;
                    stack.push(value);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    Ok(total)
}

fn index_bundle_subjects(bundle: &mut Bundle) {
    let mut subjects = BTreeMap::<String, Subject>::new();
    for subject in &bundle.subjects {
        subjects.insert(subject.id.clone(), subject.clone());
    }
    for fact in &bundle.facts {
        subjects.insert(fact.subject.id.clone(), fact.subject.clone());
    }
    for edge in &bundle.edges {
        subjects.insert(edge.from.id.clone(), edge.from.clone());
        subjects.insert(edge.to.id.clone(), edge.to.clone());
    }
    for chain in &bundle.subject_chains {
        for subject in &chain.nodes {
            subjects.insert(subject.id.clone(), subject.clone());
        }
    }
    bundle.subjects = subjects.into_values().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> ProducerProvenance {
        ProducerProvenance {
            name: "test",
            version: "1",
            adapter: "reir.adapters.test",
            adapter_version: "1",
            source: "test_fixture",
        }
    }

    fn unknown_coverage(reason: &str) -> UnknownCoverage<'_> {
        UnknownCoverage {
            id: "fact.test.unsupported".to_owned(),
            subject_kind: SubjectKind::TerraformResource,
            subject_id: "terraform::unsupported.test".to_owned(),
            subject_name: "unsupported.test".to_owned(),
            package: "terraform",
            reason: reason.to_owned(),
            source: "test",
            acquisition_mode: AcquisitionMode::TerraformPlan,
            evidence_kind: EvidenceKind::TerraformPlanPointer,
            evidence_file: "plan.json",
            evidence_pointer: Some("/resource_changes/0".to_owned()),
            evidence_value: "unsupported_resource_type",
        }
    }

    #[test]
    fn builder_enforces_operation_fact_and_string_budgets() {
        let mut operations = BoundedEvidenceBuilder::new(AdapterLimits::new(0, 1, 1_000));
        assert!(matches!(
            operations.record_operation(),
            Err(AdapterBuildError::OperationBudgetExceeded { .. })
        ));

        let mut facts = BoundedEvidenceBuilder::new(AdapterLimits::new(10, 0, 1_000));
        assert!(matches!(
            facts.push_unknown_coverage(unknown_coverage("unsupported")),
            Err(AdapterBuildError::FactBudgetExceeded { .. })
        ));

        let mut strings = BoundedEvidenceBuilder::new(AdapterLimits::new(10, 1, 8));
        assert!(matches!(
            strings.push_unknown_coverage(unknown_coverage("unsupported")),
            Err(AdapterBuildError::StringBudgetExceeded { .. })
        ));
    }

    #[test]
    fn unknown_coverage_is_explicit_and_provenance_is_complete() {
        let mut builder = BoundedEvidenceBuilder::new(AdapterLimits::new(10, 1, 10_000));
        builder
            .push_unknown_coverage(unknown_coverage("unsupported type"))
            .expect("coverage fact");
        let bundle = builder.finish(provenance()).expect("bundle");

        assert_eq!(bundle.producers.len(), 1);
        assert_eq!(
            bundle.producers[0].adapter.as_deref(),
            Some("reir.adapters.test")
        );
        assert_eq!(bundle.facts[0].value, FactValue::Unknown);
        assert_eq!(
            bundle.facts[0].unknown_reason.as_deref(),
            Some("unsupported type")
        );
        assert!(!bundle.facts[0].evidence.is_empty());
    }

    #[test]
    fn incomplete_producer_provenance_is_rejected() {
        let builder = BoundedEvidenceBuilder::new(AdapterLimits::new(1, 1, 1));
        let error = builder
            .finish(ProducerProvenance {
                source: "",
                ..provenance()
            })
            .expect_err("empty source must fail");
        assert_eq!(error, AdapterBuildError::MissingProvenance("source"));
    }
}
