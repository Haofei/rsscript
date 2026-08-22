# Changelog

RSScript has not made a tagged compatibility release. Until then, notable
changes on `main` are grouped by independent contract so early adopters can
track migrations without confusing crate versions with wire versions.

## Unreleased

### Language semantics

- No new syntax or qualifiers; the language surface remains frozen while Core
  execution boundaries converge.

### Artifact and bytecode

- `rsscript.bytecode.v1` is the sole emitted and executable bytecode contract.
- Artifact admission now has an explicit origin-verification extension point.

### Provider ABI

- Official Providers use canonical `WireValue` calls.
- Environment, process, filesystem, and HTTP authorities are instance-owned
  and fail closed.
- HTTP calls with a cancellation token now report in-flight cancellation
  promptly while the bounded blocking transport finishes on an owned worker.

### SDK and execution report

- Native JIT selection uses typed host options and no longer changes limits.
- Execution reports include actual interpreter/native engine telemetry.

### Runner protocol

- Runner response v1 carries a typed, versioned execution-report v2 envelope.

### Repository governance

- Workflow validation rejects Cargo test filters that match no declared test,
  preventing stale filtered commands from succeeding with zero tests.
- Public bug/PR templates route security reports to private disclosure and make
  trust-boundary verification explicit.
