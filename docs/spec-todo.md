# RSScript language-spec TODO

Status of the spec surface (`RSScript_v0.7_Spec.md`) beyond the executable MVP.

The spec is a deliberate **superset** of the implementation, in three tiers:

1. **Executable MVP (§3.1)** — implemented and tested at VM↔compiled parity.
2. **Review-visible but not executable (§3.2)** — parsed and surfaced for review,
   but rejected before lowering with stable diagnostics (e.g. `spawn` →
   `RS0015`/`RS0101`).
3. **Post-v0.7 deferred directions (§20.1)** — future work.

Every item below is **resolved**: shipped, shipped-by-design, or a recorded
decision. The only open work is the explicitly-optional post-v0.7 enhancements at
the end — none of it blocks v0.7.

## Shipped

- [x] **`await` in expression position** (§3.2, §20.1-A, §14.6.2). Await-hoisting
  syntax pass (`syntax/async_await_hoist.rs`): a nested `await` in a call argument,
  return value, or assignment-target index is lifted, left-to-right, to a preceding
  `let __rss_await_N = await <op>` — the linear form both backends lower. VM↔compiled
  parity (`parity_await_in_expression`). The conditionally-evaluated short-circuit
  `&&`/`||` RHS stays `RS0411` by design.

- [x] **Structured-fix tooling** (§20.1-D). `diagnostic::FixEdit { span, replacement }`
  on `Fix` (insertion/replacement; serialized into `rss check --json` /
  `rss ide --json diagnostics`); high-value machine-applicable fixes
  (`add_data_effect`, `add_constructor_field_effect`) emit real edits; `rss fix
  [--write] [--json]` applies them bottom-to-top and skips overlaps. End-to-end +
  lib-level tests. The request/response **analysis server** is `rss ide --json`
  (diagnostics/symbols/hover/definition/references/outline), now carrying fix edits.

- [x] **FFI / native-ABI adapter contracts** (§3.2, §20.1-N). Surface was already
  mature (`native fn`, `native module` grouping, transitive binding inheritance,
  unbound-call error at lowering, `rss native audit`, review capability
  classification). Closed the named gap — compact whole-boundary binding — with an
  `[adapter.<Namespace>]` section in `native/bindings.rssbind.toml` (pkg §9.5) that
  expands at load time into the same flat binding map every consumer uses (single
  expansion point, zero parity risk). Duplicates rejected; unit-tested.

- [x] **`Stream<T>` + `await for` async sequences** (§20.1-H). Already in the
  executable MVP: `await for`, `Stream<T>`, `Stream.next`, `Receiver.into_stream`,
  and built-in stream sources (`File.bytes_stream`, `Csv.rows`, `Process.stream`)
  parse, check, lower to Rust, and run in the reg-VM — VM↔compiled parity in
  `vm_eval_parity/async_concurrency.rs`. (User-defined async generators and stream
  combinators are optional post-v0.7 enhancements, below.)

- [x] **Sum-type hardening** (§20.1-E). The v0.7 baseline is complete: payload
  variants, generic sums, exhaustiveness checking, match in expression position,
  and sum-type review/diff metadata all ship and are tested. (Named payload fields
  — `Circle { radius: Int }` vs positional — are an optional post-v0.7 enhancement,
  below.)

- [x] **Registry-level review-risk badges** (§20.1-F). `PackageReview` and the
  registry index (`rss.registry.index.v1`) now carry a compact `badges` set
  (`risk:<level>`, `native`, `unsafe`, `async`, `parallel`, `unknown-capability`,
  `has-errors`) derived from the existing risk + capability summary — a restatement
  of review evidence, never new analysis. Rendering badges in a registry UI is a
  registry-side concern outside this repo.

## Shipped by design (no gap)

- [x] **Module visibility / re-export hardening** (§20.1-M). `pub` is enforced at
  the boundary that matters: the package public contract is filtered to `pub` /
  interface-declared items (`package/contract.rs`), and consumers only ever see a
  dependency's `.rssi`. Non-`pub` items are **package-private by design** (§14.8:
  "items without `pub` are package-private implementation details"), so within-
  package cross-module access is intentional, not a leak. Module aliasing, qualified
  calls, and glob imports already shipped. (`pub use` re-export is an optional
  enhancement, below.)

## Decided: out of scope for v0.7 (rationale recorded)

These are deliberate non-goals or deferred architecture, not pending work.
Building them now would contradict the v0.7 design.

- **`spawn` + public task handles** (§3.2) — _deferred by design._ The single-isolate
  cooperative model is the v0.7 stance; unstructured `spawn` is intentionally
  rejected (`RS0015`/`RS0101`) in favor of `task_group` structured concurrency.
  Revisit only if/when the isolate model gains unstructured tasks.
- **Capability objects / explicit dynamic dispatch** (§3.2, §20.1-G) — _deferred by
  design._ RSScript does not adopt `dyn Trait`/vtable coercion in v0.7 (spec marks
  dynamic dispatch "not admitted in v0.7"); static `impl` + generic bounds are the
  sanctioned mechanism. A future open-dispatch design must stay review-explicit.
- **Cross-isolate message API (zero-copy transfer)** (§20.1-B) — _deferred._ Only
  meaningful after the async/isolate model matures; depends on the isolate boundary
  design not yet settled.
- **Rust-style open-enum machinery** (§3.2) — _non-goal._ Counter to the sealed-sum
  design (which underpins exhaustiveness + review diffs). Not planned.
- **Two-tier execution (dev interpreter + AOT)** (§20.1-C) — _not a capability gap._
  The reg-VM and compiled backends already run at parity; this is a perf/packaging
  optimization, tracked with perf work, not a missing language feature.
- **Scoped views / slices (zero-copy borrowed regions)** (§3.2, §20.1-I) — _deferred._
  A genuine future perf/ergonomics feature (pairs with the parked lazy-`Iter` note
  in `TODO.md`); large enough to warrant its own design pass. Not required for v0.7.

## Optional post-v0.7 enhancements (tracked, non-blocking)

Small/medium add-ons to already-shipped features. None is required; pick up if a
real driver (the tinygrad port, dogfooding) demands it.

- **`Fix` edits for the remaining machine-applicable fixes** — extend the `rss fix`
  payload to more fix sites as they come up.
- **LSP-protocol daemon (stdio)** — a long-running server for editors; `rss ide` is
  request/response today.
- **`pub use` re-export** — let a package re-export a dependency's item as its own.
- **Sum named payload fields** — `Variant { field: T }` in addition to positional.
- **Stream combinators / user-defined async generators** — `.map`/`.filter` on
  `Stream`, custom stream sources.
