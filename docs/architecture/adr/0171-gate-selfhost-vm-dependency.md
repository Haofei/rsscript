# ADR 0171: Gate the self-host VM dependency behind its research feature

- Status: Accepted
- Date: 2026-08-14

## Problem

`rsscript-compiler` retained unused development dependencies on REIR, review
integrations, fuzz/schema tooling, and the legacy VM. This made the Core
compiler manifest appear to own research integrations even though normal code
did not use them. The self-host parity harness does require the legacy VM, but
only when its explicit research feature is selected.

## Decision and non-goals

Remove the unused compiler development dependencies. Declare `rsscript-vm`
only as an optional dependency selected by `selfhost-parity`; its existing
`legacy-exec-ir` feature remains confined to that research harness.

This does not remove the self-host harness or the legacy executable-IR bridge.
Those are migrated only after the direct MIR path reaches full parity.

## Compatibility and migration

Default compiler, SDK, CLI, Artifact, and Provider dependency closures become
smaller. The explicit command used by the self-host workflow continues to
select `--features execution,selfhost-parity`, so the research test harness
retains its required VM dependency. No public API or persisted format changes.

## Verifier and security impact

No execution or verifier behavior changes. The change prevents experimental
test/integration dependencies from being accidentally treated as compiler-Core
requirements.

## Provider and backend impact

No Provider change. REIR/review integrations stay in their experimental
workspace; self-host VM access remains an explicit research-only backend path.

## Evidence

The compiler default lib suite passes without dev dependencies. The explicit
self-host feature combination compiles with `--no-run`. Architecture tests
require an empty compiler dev-dependency table and require the optional VM to
be selected solely by `selfhost-parity`.
