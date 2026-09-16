# Changelog

RSScript has not made a tagged compatibility release. Until then, notable
changes on `main` are grouped by independent contract so early adopters can
track migrations without confusing crate versions with wire versions.

## Unreleased

### Language semantics

- Brace struct literals `T { field: value }` and comma-terminated match arms
  `Pattern => expr,` are accepted as parser sugar for the canonical spellings;
  `rss fmt` prints the canonical form. No new semantics.
- New checker rules, each with a code: `break`/`continue` outside a loop
  (RS0016), reads before assignment (RS0017), unresolvable `use` paths
  (RS0018), private declarations used from another module (RS0019), and
  `let … else` blocks that do not diverge (RS0020). `Dyn.from` now checks
  protocol conformance (RS0032), `?` requires a propagation target (RS0013),
  and RS0034 reports a generic construction whose type arguments cannot be
  proved.
- Fixed: a `loop {}` with no `break` diverges; structured and variant patterns
  agree on the scrutinee effect; `Char`, `Byte`, and every integer width are
  `Ord`; a comparison can be an `if`-expression condition or a `match`
  scrutinee; a `protocol` may be declared inside a `module`; RS0017 and RS0020
  look inside closure bodies; a `let … else` block no longer drops its first
  statement (a parser off-by-one that changed program meaning).
- Execution rules are now specified: call-argument evaluation order, checked
  `Int` arithmetic, `select` tie-breaking, the `main` signature, and
  creation-order task wake-up after a park (previously hash-dependent).
- Every program the checker accepts now builds, verifies, and runs: callback
  parameters, closures with implicit captures, bare `None`, tuple returns and
  patterns, list and multi-field variant patterns, index assignment, and the
  receiver-call spelling of mutating core methods all lower. Writing to a
  captured local, which was silently lost at run time, is now RS0805.
- `docs/spec/RSScript_Semantics_v0.7.md` is the source-backed reference for
  every rule, with a verified example and a diagnostic index.

### Artifact and bytecode

- `rsscript.bytecode.v1` is the sole emitted and executable bytecode contract.
- Artifact admission now has an explicit origin-verification extension point.
- The canonical `WireType` model gains a `Function` form carrying a callback's
  arity, per-parameter data effects, and result type, so a user function with a
  `noescape Fn(...)` parameter is an ordinary ABI position that builds, verifies,
  and runs (ADR 0234). The addition is backward compatible: the new `"function"`
  tag appears only in artifacts that use a function type.

### Provider ABI

- Official Providers use canonical `WireValue` calls.
- Environment, process, filesystem, and HTTP authorities are instance-owned
  and fail closed.
- HTTP calls with a cancellation token now report in-flight cancellation
  promptly while the bounded blocking transport finishes on an owned worker.

### SDK and execution report

- Native regions report the interpreter's exact step and intrinsic-call counts
  whether or not a limit is armed, stop for the same step-budget,
  intrinsic-budget, cancellation, and deadline reasons, honour `max_depth`,
  and meter native-to-native call edges; a region whose cost cannot be
  attributed exactly declines to the interpreter. Previously native call edges
  ran unmetered and unarmed runs reported zero steps.
- `rss run --trusted-in-process --native` keeps the default runner limit
  profile instead of substituting the unbounded trusted-host profile, and a
  `main` that calls a hot helper now reaches native code under those limits.
- The native execution report no longer panics on serialization.
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
- The native engine telemetry summary's `compile_nanos` and `run_nanos` are
  `u64` nanoseconds rather than `u128`, so a native execution report can be
  deserialized at all: an internally tagged enum buffers its content through
  serde's `Content`, which has no 128-bit carrier (ADR 0235). The JSON is
  unchanged for every representable value, and no consumer could parse the old
  shape, so there is no migration.

### CLI and tooling

- `rss build` verifies the Artifact before writing it; check, fix, build, run,
  and inspect assemble sources, the standard package interfaces, and
  `--interface` files through one shared path and report identical
  diagnostics.
- RS0206 and RS0203 carry did-you-mean suggestions drawn from the in-scope
  symbol table with machine-applicable renames; RS0202 emits a correct effect
  fix. `rss generate continuations` returns each parameter's effect and label
  facts and sees the standard package interfaces.
- The generated language card carries canonical surface forms and a full core
  signature index. The eval corpus grew to 30 tasks with real model samples and
  a measured failure-mode report.

### Repository governance

- Rust AOT, REIR review, self-hosting research, and the artifact store are
  archived on the `archive/experiments-2026-09` branch and removed from the
  workspace. Eleven small crates were merged into four; the workspace has 26
  crates.
- The module-size, allow-debt, security-debt, and test-closure registries were
  removed. The JIT is product-owned (ADR 0233) with a `product` retention
  status. ADRs 0001–0225 are archived under `docs/architecture/adr/archive/`.

- Workflow validation rejects Cargo test filters that match no declared test,
  preventing stale filtered commands from succeeding with zero tests.
- Public bug/PR templates route security reports to private disclosure and make
  trust-boundary verification explicit.
