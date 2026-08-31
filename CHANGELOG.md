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
- Native continuation dispatch now rejects non-entry instruction positions before
  constructing typed instances or writing frame state; diagnostics expose
  candidate, full-probe, and instance-key counts.
- Automatic OSR recognizes the current MIR `while` exit-trampoline shape, yields
  helper-bearing loops to continuation JIT, and has end-to-end threshold/disable
  coverage. Post-native handle materialization now uses the live VM transaction
  instead of decoding a user token as a helper-call wrapper.
- Tiered whole functions with an internal backedge start directly in optimized
  Cranelift code. Infallible scalar callees share one canonical direct body with
  a small versioned frame-ABI adapter instead of emitting two full bodies.
- VM-native flat-buffer calls bind borrow proofs to ABI slots, making validation
  linear without the per-call mutable-proof bitmap.
- The experimental native option `enable_osr` was split into
  `enable_auto_osr` (threshold-driven production behavior) and `eager_osr`
  (first-header diagnostic behavior). Pre-tag embedders using struct literals
  must rename the former field and explicitly choose whether eager probing is
  required.
- Execution reports include actual interpreter/native engine telemetry.
- Three native-JIT research surfaces were removed after their controlled
  scorecard workloads failed the experimental-retention threshold: profile-guided
  speculation (closure PIC and branch side exits), non-tail native recursion
  (whose only stack boundary was a static frame estimate, not a hard safety
  proof), and struct scalar replacement (net-negative against the interpreter).
  Their VM feature flags (`jit-speculation`, `jit-recursion-experimental`,
  `jit-struct-sr-experimental`) and the Cranelift `speculation`/`recursion`
  features no longer exist; the supported `native-jit` engine is unaffected.

### Runner protocol

- Runner response v1 carries a typed, versioned execution-report v2 envelope.

### Repository governance

- Workflow validation rejects Cargo test filters that match no declared test,
  preventing stale filtered commands from succeeding with zero tests.
- Public bug/PR templates route security reports to private disclosure and make
  trust-boundary verification explicit.
