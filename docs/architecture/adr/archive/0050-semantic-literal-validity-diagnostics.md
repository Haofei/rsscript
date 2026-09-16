# ADR 0050: Semantic ownership of literal validity diagnostics

## Status

Accepted.

## Problem

The compiler body checker diagnosed decimal integer literals that exceed
RSScript's `Int` range and `Char` literals that contain other than one Unicode
scalar. These are checked-HIR validity rules, not compiler orchestration.

## Decision

`rsscript-semantics` owns `integer_literal_range_diagnostic` and
`char_literal_scalar_diagnostic`. The compiler invokes those functions only to
append diagnostics to its existing analysis result.

## Compatibility

The diagnostic codes, spans, messages, causes, and manual fixes remain
unchanged. This moves ownership without changing parsing, lowering, artifact,
or runtime behavior.

## Security and verification

Rejecting invalid literals before lowering prevents backend-specific overflow
or truncation behavior. No verifier or Provider contract changes are required.

## Evidence

Focused semantic tests cover out-of-range integers and multi-scalar characters.
Architecture tests prevent the compiler from reintroducing the rule.
