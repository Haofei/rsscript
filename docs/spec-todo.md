# RSScript language-spec TODO

Unimplemented surface of the language spec (`RSScript_v0.7_Spec.md`), prioritized.

The spec is a deliberate **superset** of the implementation, in three tiers:

1. **Executable MVP (§3.1)** — implemented and tested at VM↔compiled parity.
2. **Review-visible but not executable (§3.2)** — parsed and surfaced for review,
   but rejected before lowering with stable diagnostics (verified: `spawn` →
   `RS0015`/`RS0101`, `await`-in-argument → `RS0411`). This file tracks these.
3. **Post-v0.7 deferred directions (§20.1)** — future work (items J/K/L are
   already done).

Priority = value to real programs / the active tinygrad port + dogfooding,
weighed against effort. Priorities are a recommendation; re-rank against the
actual driver (see "Basis" at the bottom).

## P1 — highest leverage (unblocks real code / dev loop)

- [ ] **`await` in expression position** (§3.2, §20.1-A) — _effort: medium._
  The one *executable* gap: `f(x: await g())` is rejected (`RS0411`); `await` only
  works at statement boundaries and in `if`/`loop`/`match`/`with` bodies.
  Approach: an await-hoisting (A-normal-form) desugar that lifts a nested await to
  a preceding `let __rss_await_N = await <op>` — producing the linear awaits both
  backends already lower. Keep it sound: don't hoist across short-circuit
  (`&&`/`||`) RHS, `match`/`if` arms, or closure bodies. Verify at VM↔compiled
  parity.

- [ ] **Structured-fix tooling + analysis server** (§20.1-D) — _effort: medium–large._
  `rss fix` applying machine-applicable structured fixes (the `Fix`/applicability
  infrastructure already exists on diagnostics), and a language/analysis server
  streaming diagnostics + fixes (build on the existing `lsp` crate). Biggest
  dev-experience multiplier; helps every user and the port edit→check loop.
  `rss fix` is the bounded sub-part; the full LSP is the larger half.

- [ ] **FFI / native-ABI adapter contracts** (§3.2 general FFI, §20.1-N) —
  _effort: medium–large, open-ended._
  `native fn` declares external boundaries today; the gap is binding *whole*
  runtime/autogen/device boundaries compactly without large wrapper files, plus
  ABI-adapter conformance facts. Needs a concrete target boundary to scope "done."

## P2 — real features, not blocking today

- [ ] **Capability objects / explicit dynamic dispatch** (§3.2, §20.1-G) —
  _large._ Protocol-typed values / open dispatch; today only static `impl` +
  generic bounds.
- [ ] **`Stream<T>` + `await for` async sequences** (§20.1-H) — _medium–large._
  Async iteration; `rss-async` has the base. Pairs with extended async (A).
- [ ] **Scoped views / slices** (zero-copy borrowed regions) (§3.2, §20.1-I) —
  _large._ Perf + ergonomics; pairs with the parked lazy-`Iter` note in `TODO.md`.
- [ ] **Sum-type hardening** (§20.1-E) — _small–medium._ Incremental tightening of
  the shipped sum surface.
- [ ] **Module visibility / re-export hardening** (§20.1-M) — _small–medium._
  Refines the module system (aliasing/qualified/glob already shipped).

## P3 — deferred by design / lower value

- [ ] **`spawn` + public task handles** (§3.2) — _large._ Single-isolate
  cooperative model is the deliberate v0.7 stance; unstructured `spawn` is
  intentionally rejected for now.
- [ ] **Cross-isolate message API (zero-copy transfer)** (§20.1-B) — _large._
  Only after the async/isolate model matures.
- [ ] **Two-tier execution (dev interpreter + AOT)** (§20.1-C) — _large._ reg-VM +
  compiled already run at parity; this is an optimization, not a new capability.
- [ ] **Rust-style open-enum machinery** (§3.2) — counter to the sealed-sums
  design; likely stays a non-goal.
- [ ] **Registry-level review-risk badges** (§20.1-F) — _small–medium._ Registry
  polish.

## Basis for the ranking

- **tinygrad port driver:** P1 FFI + `await`-in-expression matter most; capability
  objects (P2) next if the port hits protocol-typed dispatch.
- **dogfooding / self-host driver:** the analysis server / `rss fix` (P1-D) is the
  biggest force-multiplier.
- **perf driver:** scoped views/slices (P2-I) + the parked lazy-`Iter` (`TODO.md`)
  move up to P1.
