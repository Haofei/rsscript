# ADR 0198: Quarantine compiler package, review, and AOT exports

- Status: Accepted
- Date: 2026-08-14

## Problem

The compiler's top-level public API exposed package traversal, persistence,
review/risk presentation, and generated Rust/AOT helpers next to the frontend
analysis and provider-neutral lowering contracts. That made historical paths
look like supported compiler primitives and let new callers widen the compiler
boundary accidentally.

Moving those implementations immediately would couple an API cleanup to the
larger project/review/AOT extraction work.

## Decision

All package, review, and Rust AOT exports live under
`rsscript_compiler::compatibility`. The reviewed top-level compiler surface
continues to expose frontend analysis, syntax, diagnostics, and normal lowering
only. SDK migration exports and the experimental CLI AOT path must import the
explicit compatibility namespace.

This is a source-compatibility break for direct users of the unpublished
compiler crate. RSScript is pre-1.0 and the old paths were already marked as
compatibility/experimental, so preserving an implicit top-level alias would
undermine the boundary this change establishes.

## Consequences

The compatibility module is temporary, not a new stable product API. It makes
the remaining extraction work mechanically visible: package/review/AOT code
can be removed only after its compatibility exports have dedicated owners.
