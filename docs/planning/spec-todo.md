# RSScript language-spec TODO

Tracks the spec surface (`RSScript_v0.7_Spec.md`) beyond the executable MVP.

The executable MVP (§3.1) is implemented and tested at VM↔compiled parity. The
**open items** below are the committed in-scope roadmap (spec §20.2). Everything
else is either already shipped or a recorded non-goal.

## Open items — in scope, to implement (spec §20.2)

Four committed, design-compatible directions. All are large; each extends the
model without reversing a review-first tenet. Ordered by readiness/value.

- [x] **Two-tier execution: dev interpreter + AOT** (§20.1-C, §20.2-4) — _done._
  `rss dev --run` now routes through the **reg-VM dev tier** by default (fast inner
  edit→run loop, no rustc) and switches to the **Rust-lowering AOT tier** with
  `--release`. Both tiers observe identical semantics/diagnostics (the existing
  VM↔compiled parity invariant), so the fast tier is trustworthy. Single files run
  through the source VM; package directories through the package VM with native host
  bindings loaded. Measured: ~54 ms (VM) vs ~14 s (AOT) for the same program.
  Tested: tier-label unit test + `cli_dev_two_tier.rs` end-to-end.

- [x] **Scoped views / slices** (zero-copy borrowed regions) (§3.2, §20.1-I, §20.2-1)
  — _done._ The borrowed-view *types* already existed and ran at parity
  (`String.view`/`StringView`, `Bytes.view`/`BytesView`, `Buffer.view`/`BufferView`,
  lowered to Rust slices). Added the spec's dedicated **`view name = expr`** form: a
  parser-level desugar turns `view v = e; <rest>` into `with e as v { <rest> }`, so
  the view's scope ends at the enclosing block and it inherits every `with`-lease
  rule — cannot escape (return), be retained, cross `await`, or enter a managed
  graph — with **no new borrow analysis and zero changes below the parser** (so
  VM↔compiled parity is automatic). Verified: `parity_scoped_view_binding` (incl. a
  view-of-a-view) and the `scoped-view-escape.rss` fail fixture (`RS0702`). The
  `with Buffer.view(...) as v { … }` form also already works.

- [x] **Capability objects / explicit dynamic dispatch** (§3.2, §20.1-G, §20.2-2) —
  _done._ The compiled backend already lowered `Capability<Protocol>` to a
  closed-world enum-of-impls with `match` dispatch (no `dyn`); this completes the
  feature: (1) the spec's `capability Protocol` keyword form now parses as sugar for
  `Capability<Protocol>` (parser desugar), so `who: read capability Greeter` and
  `local x: capability Greeter` work; (2) **fixed a real VM parity bug** — the reg-VM
  had *no* runtime dynamic dispatch, so any `Protocol.method(self: x, …)` call
  (capability objects *and* generic bounds) returned `Unit` instead of dispatching.
  Added a `CallDynamic` instruction + `Hir::protocol_method_targets` that selects the
  concrete impl by the receiver's runtime struct type, mirroring the compiled enum
  dispatch. Verified at parity (`parity_capability_dynamic_dispatch`, two impls). The
  review map already flags dynamic dispatch (`has_dynamic_protocol_dispatch`).

- [~] **Cross-isolate message API (zero-copy transfer)** (§20.1-B, §20.2-3) —
  _message contract landed; multi-isolate execution still future._ The reviewable
  core — the **message payload contract** — is implemented: `Channel.message<T>`
  creates a channel whose payload `T` is statically verified
  **cross-isolate-transferable** (`is_cross_isolate_transferable`: a self-contained
  value with no managed handle — v1 allows Copy scalars, `String`, `Bytes`),
  rejecting managed/container payloads at check time (`RS0036`). It reuses the
  bounded-channel runtime, so it runs today at VM↔compiled parity
  (`parity_message_channel_roundtrip`) and is the forward-compatible transport for
  real isolates: nothing crossing it ever shares mutable state. This realizes the
  "message is the right choice for a solo isolate" decision — message semantics +
  `take` move-in, no reference-capability machinery.
  _Remaining (future):_ (a) broaden the payload contract to data-only structs/sums
  and owned containers via deep-snapshot; (b) a structured isolate-spawn so two
  isolates actually run with disjoint heaps. True multi-thread/multi-heap isolates
  are a separate multi-quarter refactor. Full plan in
  [cross-isolate-design.md](cross-isolate-design.md).

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
