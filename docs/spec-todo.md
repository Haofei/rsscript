# RSScript language-spec TODO

Tracks the spec surface (`RSScript_v0.7_Spec.md`) beyond the executable MVP.

The executable MVP (§3.1) is implemented and tested at VM↔compiled parity. The
**open items** below are the committed in-scope roadmap (spec §20.2). Everything
else is either already shipped or a recorded non-goal.

## Open items — in scope, to implement (spec §20.2)

Four committed, design-compatible directions. All are large; each extends the
model without reversing a review-first tenet. Ordered by readiness/value.

- [ ] **Two-tier execution: dev interpreter + AOT** (§20.1-C, §20.2-4) — _medium._
  The reg-VM and the Rust-lowering backend already run at parity; commit the fast
  HIR-level dev loop as a first-class tier (fast edit→run without rustc cost). Both
  tiers must keep identical semantics + diagnostics (already the parity invariant).
  Closest to done — mostly surfacing/packaging the existing reg-VM path as the
  sanctioned dev tier. _Extension points:_ `reg_vm`, the `rss eval`/`rss dev` CLI.

- [ ] **Scoped views / slices** (zero-copy borrowed regions) (§3.2, §20.1-I, §20.2-1)
  — _large._ Lexically-scoped borrowed views — `with Buffer.view(...) as bytes { … }`
  and `view bytes = …` — that cannot be retained, escape scope, cross an `await`, or
  enter managed graphs. No surface lifetimes (the scope block replaces them). High
  perf value (parsing, buffers, the tinygrad port; pairs with the parked lazy-`Iter`
  in `TODO.md`). _Touches:_ parser, the escape/retention checks (`checks/local.rs`),
  lowering, reg-VM. Must hold VM↔compiled parity.

- [ ] **Capability objects / explicit dynamic dispatch** (§3.2, §20.1-G, §20.2-2) —
  _large._ Review-visible `capability`-bounded dispatch
  (`store: read capability Store<T>`), **not** Rust-style implicit `dyn` coercion
  (which stays a non-goal, §21). The review map must flag every capability boundary;
  capability values carry their protocol's effect declarations (no silent widening).
  _Touches:_ parser/types, protocol resolution, lowering (vtable-free representation),
  review map / REIR. Must hold parity.

- [ ] **Cross-isolate message API (zero-copy transfer)** (§20.1-B, §20.2-3) —
  _large._ Typed send/receive channels between isolates; payloads are Copy/owned data
  or values moved with `take`; managed handles never cross. Single ownership enforced
  statically. Depends on the isolate model maturing first. _Touches:_ async/isolate
  runtime, type/effect checking for `take`-across-boundary, lowering + reg-VM.

## Removed — non-goals (not deferred; deleted from the roadmap)

These contradict core RSScript principles and were removed from the spec rather
than kept as future work (spec §20.2 "Removed, not deferred"; §21 non-goals).

- **Unstructured `spawn` / public task handles** — breaks the single-isolate,
  structured-concurrency model; `task_group` is the sanctioned form. The compiler
  continues to reject `spawn` with a stable diagnostic (`RS0015`/`RS0101`).
- **Rust-style open enums / open sum extension** — breaks the sealed-sum guarantees
  that make match-exhaustiveness and review-diffs sound. Sealed `sum` is the model.

## Shipped

- [x] **`await` in expression position** (§14.6.2) — await-hoisting pass; VM↔compiled
  parity. Short-circuit `&&`/`||` RHS stays `RS0411` by design.
- [x] **Structured-fix tooling** (§20.1-D) — `FixEdit` payload + `rss fix
  [--write] [--json]`; request/response analysis server is `rss ide --json`.
- [x] **FFI / native-ABI adapter contracts** (§20.1-N) — compact
  `[adapter.<Namespace>]` whole-boundary binding (pkg §9.5), single expansion point.
- [x] **`Stream<T>` + `await for`** (§20.1-H) — in the executable MVP; parity-tested.
- [x] **Sum-type hardening** (§20.1-E) — v0.7 baseline complete (payload variants,
  generics, exhaustiveness, review/diff metadata).
- [x] **Registry-level review-risk badges** (§20.1-F) — `badges` on the package
  review + registry index, derived from risk + capability summary.
- [x] **Module visibility** (§20.1-M) — `pub` enforced at the package boundary;
  non-`pub` is package-private by design (§14.8).

## Optional post-v0.7 enhancements (non-blocking)

Small/medium add-ons to shipped features; pick up if a real driver demands it.

- **`Fix` edits for the remaining machine-applicable fixes** (extend `rss fix`).
- **LSP-protocol daemon (stdio)** — `rss ide` is request/response today.
- **`pub use` re-export** — re-export a dependency's item as a package's own.
- **Sum named payload fields** — `Variant { field: T }` alongside positional.
- **Stream combinators / user-defined async generators** — `.map`/`.filter`, custom
  stream sources.
