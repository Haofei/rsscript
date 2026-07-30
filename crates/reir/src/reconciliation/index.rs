use std::collections::HashMap;

use crate::{Capability, CapabilityCategory, Fact};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ExactCapabilityKey {
    category: String,
    provider: String,
    service: String,
    action: String,
    resource: String,
    constraints: Vec<(String, String)>,
}

pub(super) struct CapabilityIndex<'a> {
    by_category: HashMap<String, Vec<&'a Fact>>,
    exact: HashMap<ExactCapabilityKey, Vec<&'a Fact>>,
    broad_by_category: HashMap<String, Vec<&'a Fact>>,
}

impl<'a> CapabilityIndex<'a> {
    pub(super) fn new(facts: &[&'a Fact]) -> Self {
        let mut index = Self {
            by_category: HashMap::new(),
            exact: HashMap::new(),
            broad_by_category: HashMap::new(),
        };
        for fact in facts {
            let capability = fact
                .capability
                .as_ref()
                .expect("indexed fact should have capability");
            let category = capability_category_key(&capability.category);
            index
                .by_category
                .entry(category.clone())
                .or_default()
                .push(*fact);
            if let Some(key) = exact_capability_key(capability) {
                index.exact.entry(key).or_default().push(*fact);
            } else {
                index
                    .broad_by_category
                    .entry(category)
                    .or_default()
                    .push(*fact);
            }
        }
        index
    }

    pub(super) fn candidates(&self, capability: &Capability) -> Vec<&'a Fact> {
        let category = capability_category_key(&capability.category);
        let Some(key) = exact_capability_key(capability) else {
            return self.by_category.get(&category).cloned().unwrap_or_default();
        };
        let exact = self.exact.get(&key).map(Vec::as_slice).unwrap_or_default();
        let broad = self
            .broad_by_category
            .get(&category)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut candidates = Vec::with_capacity(exact.len() + broad.len());
        candidates.extend_from_slice(exact);
        candidates.extend_from_slice(broad);
        candidates
    }
}

fn exact_capability_key(capability: &Capability) -> Option<ExactCapabilityKey> {
    let provider = exact_dimension(capability.provider.as_deref())?;
    let service = exact_dimension(capability.service.as_deref())?;
    let action = exact_dimension(capability.action.as_deref())?;
    let resource = exact_resource_dimension(capability.resource.as_deref())?;
    // Constraint intersection is not equality-based (a missing key may still
    // intersect), so constrained capabilities remain in the broad bucket.
    if !capability.constraints.is_empty() {
        return None;
    }
    Some(ExactCapabilityKey {
        category: capability_category_key(&capability.category),
        provider: provider.to_owned(),
        service: service.to_owned(),
        action: action.to_owned(),
        resource: resource.to_owned(),
        constraints: Vec::new(),
    })
}

fn exact_dimension(value: Option<&str>) -> Option<&str> {
    value.filter(|value| *value != "*")
}

fn exact_resource_dimension(value: Option<&str>) -> Option<&str> {
    exact_dimension(value).filter(|value| !value.ends_with('*'))
}

fn capability_category_key(category: &CapabilityCategory) -> String {
    category.clone().into()
}
