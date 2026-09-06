//! Candidate-list normalization and call-surface scanning.

use std::collections::BTreeMap;

use super::*;

pub(super) fn retain_matching(candidates: &mut Vec<SemanticCompletion>, prefix: &str) {
    candidates.retain(|candidate| candidate.name.starts_with(prefix));
}

pub(super) fn deduplicate_shadowed(candidates: &mut Vec<SemanticCompletion>) {
    let mut best = BTreeMap::<(String, SemanticCompletionKind), SemanticCompletion>::new();
    for candidate in candidates.drain(..) {
        let key = (candidate.name.clone(), candidate.kind);
        match best.get(&key) {
            Some(previous) if previous.scope_depth >= candidate.scope_depth => {}
            _ => {
                best.insert(key, candidate);
            }
        }
    }
    *candidates = best.into_values().collect();
}

pub(super) fn unmatched_open_paren(source: &str, cursor: usize) -> Option<usize> {
    let mut stack = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in source[..cursor].char_indices() {
        if in_string {
            if !escaped && ch == '"' {
                in_string = false;
            }
            escaped = !escaped && ch == '\\';
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        match ch {
            '(' => stack.push(index),
            ')' => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.pop()
}

pub(super) fn split_top_level(source: &str, separator: char) -> Vec<&str> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut output = Vec::new();
    for (index, ch) in source.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if ch == separator && depth == 0 {
            output.push(&source[start..index]);
            start = index + ch.len_utf8();
        }
    }
    output.push(&source[start..]);
    output
}
