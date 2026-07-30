use std::collections::HashMap;

use crate::Capability;

pub(super) fn capability_covers(granted: &Capability, required: &Capability) -> bool {
    granted.category == required.category
        && optional_field_covers(granted.provider.as_deref(), required.provider.as_deref())
        && optional_field_covers(granted.service.as_deref(), required.service.as_deref())
        && action_covers(granted.action.as_deref(), required.action.as_deref())
        && resource_covers(granted.resource.as_deref(), required.resource.as_deref())
        && constraints_cover(&granted.constraints, &required.constraints)
}

pub(super) fn capability_intersects(left: &Capability, right: &Capability) -> bool {
    left.category == right.category
        && optional_field_intersects(left.provider.as_deref(), right.provider.as_deref())
        && optional_field_intersects(left.service.as_deref(), right.service.as_deref())
        && optional_field_intersects(left.action.as_deref(), right.action.as_deref())
        && resource_intersects(left.resource.as_deref(), right.resource.as_deref())
        && constraints_intersect(&left.constraints, &right.constraints)
}

fn optional_field_intersects(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, _) | (_, None) | (Some("*"), _) | (_, Some("*")) => true,
        (Some(left), Some(right)) => left == right,
    }
}

fn resource_intersects(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (None, _) | (_, None) | (Some("*"), _) | (_, Some("*")) => true,
        (Some(left), Some(right)) => {
            let left_prefix = left.strip_suffix('*').unwrap_or(left);
            let right_prefix = right.strip_suffix('*').unwrap_or(right);
            if left.ends_with('*') || right.ends_with('*') {
                left_prefix.starts_with(right_prefix) || right_prefix.starts_with(left_prefix)
            } else {
                left == right
            }
        }
    }
}

fn constraints_intersect(left: &HashMap<String, String>, right: &HashMap<String, String>) -> bool {
    left.iter().all(|(key, left_value)| {
        right.get(key).is_none_or(|right_value| {
            left_value == "*" || right_value == "*" || left_value == right_value
        })
    })
}

pub(super) fn capability_key_compatible(granted: &Capability, required: &Capability) -> bool {
    granted.category == required.category
        && optional_field_covers(granted.provider.as_deref(), required.provider.as_deref())
        && optional_field_covers(granted.service.as_deref(), required.service.as_deref())
        && action_covers(granted.action.as_deref(), required.action.as_deref())
        && resource_covers(granted.resource.as_deref(), required.resource.as_deref())
}

fn constraints_cover(
    granted: &HashMap<String, String>,
    required: &HashMap<String, String>,
) -> bool {
    required.iter().all(|(key, required_value)| {
        granted
            .get(key)
            .is_some_and(|value| value == "*" || value == required_value)
    }) && granted.keys().all(|key| required.contains_key(key))
}

fn optional_field_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        // The requirement does not constrain this dimension.
        (_, None) => true,
        // An explicit wildcard grant covers any specific requirement.
        (Some("*"), Some(_)) => true,
        (Some(granted), Some(required)) => granted == required,
        // A grant that does not name this field is UNKNOWN, not a wildcard: it
        // cannot prove it covers a requirement that names a specific value.
        (None, Some(_)) => false,
    }
}

fn action_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        // An explicit wildcard grant covers any required action.
        (Some("*"), _) => true,
        (Some(granted), Some(required)) => granted == required,
        // Both unconstrained.
        (None, None) => true,
        // A grant with no action is unknown, not a wildcard — it does not cover a
        // requirement that names a specific action.
        (None, Some(_)) => false,
        // A grant scoped to a specific action does not cover an unconstrained
        // (broad) requirement.
        (Some(_), None) => false,
    }
}

fn resource_covers(granted: Option<&str>, required: Option<&str>) -> bool {
    match (granted, required) {
        // Both unconstrained.
        (None, None) => true,
        // A grant with no resource is unknown, not a wildcard — it does not cover
        // a requirement that names a specific resource.
        (None, Some(_)) => false,
        // A narrow grant cannot prove coverage of a broad requirement.
        (Some(_), None) => false,
        (Some(granted), Some(required)) if granted == required => true,
        // Explicit prefix wildcard, e.g. `arn:aws:s3:::bucket/*`.
        (Some(granted), Some(required)) if granted.ends_with('*') => {
            required.starts_with(&granted[..granted.len() - 1])
        }
        (Some(_), Some(_)) => false,
    }
}
