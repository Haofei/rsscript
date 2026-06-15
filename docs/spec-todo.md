# RSScript language-spec TODO

Unimplemented surface of the language spec (`RSScript_v0.7_Spec.md`), prioritized.

The spec is a deliberate **superset** of the implementation, in three tiers:

1. **Executable MVP (§3.1)** — implemented and tested at VM↔compiled parity.
2. **Review-visible but not executable (§3.2)** — parsed and surfaced for review,
   but rejected before lowering with stable diagnostics (verified: `spawn` →
   `RS0015`/`RS0101`, `await` in a short-circuit `&&`/`||` RHS → `RS0411`). This
   file tracks these.
3. **Post-v0.7 deferred directions (§20.1)** — future work (items J/K/L are
   already done).

Priority = value to real programs / the active tinygrad port + dogfooding,
weighed against effort. Priorities are a recommendation; re-rank against the
actual driver (see "Basis" at the bottom).

## P1 — highest leverage (unblocks real code / dev loop)

- [x] **`await` in expression position** (§3.2, §20.1-A, §14.6.2) — _done._
  Implemented as an await-hoisting (A-normal-form) syntax pass
  (`syntax/async_await_hoist.rs`, run in `isolate_module_namespaces` for every
  backend): a nested `await` in a call argument, return value, or assignment-target
  index is lifted, in left-to-right order, to a preceding
  `let __rss_await_N = await <op>` — the linear form both backends already lower.
  Verified at VM↔compiled parity (`parity_await_in_expression`). The one remaining
  non-linear position, the short-circuit `&&`/`||` RHS, stays rejected (`RS0411`)
  to preserve conditional-evaluation semantics.

- [x] **Structured-fix tooling + analysis server** (§20.1-D) — _structured-fix
  tooling done; streaming LSP daemon remains._
  Added a concrete edit payload `diagnostic::FixEdit { span, replacement }` on
  `Fix` (insertion or replacement; serialized into `rss check --json` /
  `rss ide --json diagnostics`), wired the high-value machine-applicable fixes
  (`add_data_effect`, `add_constructor_field_effect`) to emit real edits, and
  shipped `rss fix [--write] [--json]` which plans the edits, applies them
  bottom-to-top (so positions don't shift) and skips overlaps. Verified end-to-end
  (`cli_fix.rs`: missing `read` → clean check) plus a lib-level edit-generation
  guard. The request/response **analysis server** already exists as `rss ide --json`
  (diagnostics/symbols/hover/definition/references/outline) and now carries the fix
  edits too.
  _Remaining:_ (a) populate edits for the other machine-applicable fixes as they
  come up; (b) a persistent **LSP-protocol daemon** (stdio) for editors — `rss ide`
  is request/response, not a long-running server. Track as a follow-up.

- [x] **FFI / native-ABI adapter contracts** (§3.2 general FFI, §20.1-N) —
  _compact whole-boundary binding done; deeper adapter protocol remains._
  The FFI surface was already mature (`native fn`, `native module` grouping,
  transitive binding inheritance, unbound-call error at lowering, `rss native
  audit`, review capability classification). Closed the named gap — "binding whole
  boundaries compactly without large wrapper files" — with an
  `[adapter.<Namespace>]` section in `native/bindings.rssbind.toml` (package-mgr
  §9.5): one `crate` + a `functions` list (plus optional `rename`) binds a whole
  namespace, expanding at load time into the same flat `symbol -> target` map every
  consumer already uses (lowering, VM shim, conformance checks) — **zero parity
  risk, single expansion point** (`flatten_native_bindings`). Duplicates across an
  adapter and explicit `[bindings]` are rejected. Unit-tested for expansion,
  rename, composition, and equivalence to the explicit form.
  _Remaining (deferred, §20.1-N):_ deeper structured adapter protocol (Rust adapter
  crate per dep), broader binding-conformance facts, dependency updates as semantic
  review events. General FFI / C-header parsing / auto-binding stay non-goals.

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
