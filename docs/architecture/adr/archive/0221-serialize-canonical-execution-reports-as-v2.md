# ADR 0221: Serialize canonical execution reports as v2

- Status: Accepted
- Date: 2026-08-15

## Problem

The reviewed Rust SDK already exposes a mutually-exclusive `ExecutionOutcome`
and canonical `WireValue` result. Its default JSON serializer nevertheless
emitted `rsscript.execution_report.v1`, including a legacy dynamic
`NativeValue` field. This made the machine contract weaker than the reviewed
Rust API and kept an obsolete escape representation on the normal runner path.

## Decision

The reviewed SDK emits `rsscript.execution_report.v2`. The document has one
`outcome` object:

* `completed` carries an optional canonical `WireValue` plus display text; or
* `failed` carries the structured `RuntimeError`.

The v2 schema has no `NativeValue` field. Version 1 schemas and fixtures remain
checked in as historical compatibility evidence, but the reviewed SDK no longer
emits a v1 report. VM internals may retain `NativeValue` only at an explicit
legacy value-adapter boundary while the remaining Provider migration completes.

## Consequences

New runner and host consumers receive a typed result without parsing display
text or dynamic field/type names. Consumers pinned to v1 must retain their
existing reader or explicitly migrate to v2; this is an intentional pre-1.0
schema transition. Script failures, cancellations, deadlines, and budget
exhaustion still return a complete report rather than an outer error.
