# ADR 0212: Bind checked source function contracts to Artifact analysis

- Status: Accepted
- Date: 2026-08-14

## Problem

`SemanticDiffV2` already understands function parameters, `read`/`mut`/`take`,
return contracts, and `retains(...)`. Those facts were available from package
analysis, but ordinary direct source builds only carried call facts. Consumers
therefore had to opt into the legacy package path to diff a checked function's
ownership contract.

## Decision

`SourceAnalysisV1` now contains canonically ordered `ExportFactV1` entries for
checked source function bodies. Each entry records the canonical function name,
sync/async shape, parameter effect/type/retention, return type, retained
parameter names, and neutral facts for `mut`, `take`, `retains`, `fresh`, and
async boundaries. The records are derived from the same validated HIR that
produces bytecode; neither semantic diff nor Artifact consumers reparse source
text.

The source schema adds this as an optional, default-empty v1 field. Existing
source Bundles therefore remain readable, while newly emitted Bundles carry
the evidence required for direct ownership and retention diffs.

## Non-goals

This does not make source analysis a review or authorization mechanism, expose
Provider implementations, or claim that direct source analysis contains every
package-level fact. Resource-lifetime, task, recursive, and transitive package
facts remain separate extensions of their corresponding semantic evidence.

## Evidence

Artifact unit tests retain strict typed source-analysis decoding. SDK tests
compile two direct source Artifacts whose function changes from `mut` to
`read` plus `retains(value)`, then prove both typed evidence and
`SemanticDiffV2` report the contract change.
