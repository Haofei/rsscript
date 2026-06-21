# Value-representation plan — unbox to kill allocation and unlock the JIT

Make the reg-VM fundamentally faster by **changing the value representation so the
interpreter stops heap-allocating value-semantic types**, then use that to unlock
the Cranelift tier to keep hot-loop values in registers and elide allocation —
**without breaking the §2 parity invariant** in
[`docs/spec/RSScript_Execution_Spec_v0.1.md`](../spec/RSScript_Execution_Spec_v0.1.md),
and **without AOT** (this VM is for local development; startup must stay instant).

Companion to [`vm-jit-perf-plan.md`](./vm-jit-perf-plan.md). That plan exhausted
the *micro* wins (dispatch inline +14–30%, float reads, closure cache +9.5%); this
plan is the *structural* leap those can't reach.

> **Keep this doc — it is the evidence base, not dead.** V1 + V2.0 are **landed on
> `main`** (this is their rationale). V2.1/V2.2/V3 are **evidenced rejections** (the
> uniform-`VmValue` slot can't inline composites without a size tax). That refutation
> is exactly what motivates the *successor* direction — **flatten typed *containers*,
> not the uniform slot** — pursued in
> [`vm-valhalla-plan.md`](./vm-valhalla-plan.md). Read V2.1's failure here, then the
> Valhalla plan for the way forward. V4/V5 (JIT-side) live in
> [`vm-optimizing-jit-plan.md`](./vm-optimizing-jit-plan.md).

Status: **in progress — V1 + V2.0 landed (soak-green wins); V2.1/V2.2 rejected by
measurement.** Net result on `main`: **V1 (Option scalar unbox)** `match_option_loop`
−5%, size 16→16; **V2.0 (layout interner)** `variant_match_loop` −17.8%,
`nested_struct_field` −47.5%, `VmStruct` 48→32 — both full-soak-verified. **V2.1
(variant field SVO) implemented + soak-green but REJECTED**: net-negative (+29%
scalar loops from `VmValue` 16→24; +62% `option_result_chain`) because composites
need a layout pointer that blows the 16B budget, and V2.0 already captured the
composite win. **V2.2 skipped** (same problem). The validated thesis: unboxing pays
where it fits 16B (Option) and the *layout interner* is the big composite win;
field-inlining composites does not pay. **V3 (collections COW) measured
NOT-WARRANTED** (collection alloc is HashMap-node/internal, not `deep_copy` — COW
wouldn't help). Full-suite re-baseline confirms **36/37 interpreter cohorts
improved or neutral** vs the pre-session committed baseline (standouts
`nested_struct_field` −54%, `map_string_keys` −60%, `variant_match_loop` −32%;
controlled re-measure shows the 2 full-suite "regressions" were load noise —
`match_option_loop` −21%, `option_result_chain` −34% in isolation). Interpreter-side
value-rep work is **complete**; V4/V5 are JIT-side (companion optimizing-JIT plan,
partly mooted by the composite-unboxing rejection). Owner: TBD. Created 2026-06-20.

---

## 1. Problem & thesis

`vm-jit-perf-plan.md` §0.3/§4.1 established, with Callgrind, that on the real
(alloc-bound) cohorts **~40% of all retired instructions are inside the libc
allocator** (`malloc`/`free`) and ~7% more in `Rc` drop/clone. The worst kernels
are 60–651× slower than the compiled-Rust ceiling: `option_result_chain` 651×,
`nested_struct_field` 365×, `variant_match_loop` 298×. Dispatch (what the inline
fixed) and native coverage (float reads) are real but secondary; **allocation +
refcount churn is the dominant cost.**

The root cause, in JVM terms: **the VM behaves like a JVM where every value is
boxed** — every `Int`, every `Some(x)`, every struct/variant is a heap object
(`Rc<…>`, an object with a refcount). It is *autoboxing in the hot loop*, for all
values, always. `vm_value.rs:52` — `VmValue` is a 16-byte tag with
`OptionSome(Box<VmValue>)`, `Struct(Rc<VmStruct>)`, `Variant(Rc<VmStruct>)`,
`List(Rc<RefCell<Vec>>)`. Every `Some`/`MakeStruct`/`MakeVariant`/`MakeList`
allocates.

**Thesis:** unbox the value-semantic types (Valhalla-style inline values) so the
common case stops allocating. This (a) directly attacks the 40%, benefiting *all*
code with no JIT and no compile step (dev-wide), and (b) makes values
*register-shaped*, which is the prerequisite for the Cranelift tier to do
escape-analysis/scalar-replacement and elide allocation on hot loops (§8).
Expected: **3–10×** on the alloc-bound cohorts at the interpreter level, more on
hot loops once the JIT consumes the new representation.

**Why this is safe where the closure cache was not.** Verified in `vm_value.rs`
`PartialEq` (~line 455): `Struct`/`Variant`/`List`/`Map`/`Option` all compare
**structurally** (by name + fields / by value). **Only `Closure` uses
`Rc::ptr_eq`** (reference identity). So for value-semantic types the `Rc` is *pure
implementation* — its identity is unobservable — and re-representing them is
parity-transparent. Closures are the one reference-semantic type (JVM: a normal
object whose `==` is reference equality) and are **explicitly out of scope** here.
This is the Valhalla distinction: value classes (no identity) vs identity classes.

## 2. JVM framing (the mental model)

| This plan | JVM equivalent |
|---|---|
| Unbox value-semantic structs/variants/Option | **Project Valhalla** inline/value classes (`int` not `Integer`) |
| JIT keeps non-escaping unboxed values in registers | **C2 escape analysis + scalar replacement** |
| Hot loop in a once-called fn never tiers up (todo) | the case **OSR** exists to fix |
| Cold interpreted, only hot compiled | **HotSpot tiered compilation** |
| No AOT because it's a dev VM | not using `native-image`; want `java`-style instant start |

The two halves: **Valhalla (representation)** is the interpreter win and the
enabler; **escape analysis + OSR (compiler)** is how Cranelift cashes it in.

## 3. Hard constraints (do not regress)

- **§2 Parity invariant.** Every tier — HIR-interp, reg-VM, tier-0, native,
  native-force-deopt, **compiled-Rust** — must stay observably identical. A
  value-rep change is exactly what the differential guards, so the **full
  generative soak is MANDATORY per slice** (not the fast gate alone):
  `docker compose run --rm dev bash -lc 'CARGO_NET_OFFLINE=true RSSCRIPT_FULL_BACKEND_PARITY=1 RSS_DIFF_PROPTEST_CASES=200 RSS_GENERATIVE_CASES=64 RSS_GENERATIVE_MUTATION_CASES=200 cargo test -p rsscript --test differential'`
  (~20 min, compiled backend included). Fast gates (`runtime jit_acceptance`,
  `--test differential`) are the inner loop; the soak is the slice-exit gate.
- **Value-semantic types ONLY.** `Struct`, `Variant`, `Option`, `List`, `Map`,
  `Deque`, `Bytes`, `String`, `Json` (structural identity). **`Closure` is
  untouched** (reference identity via `Rc::ptr_eq`, `vm_value.rs:471`). `Native`
  and `Managed` are out of scope (identity / interior-mutability semantics — audit
  before touching).
- **Canonical representation (no aliasing of representations).** A given logical
  value must have exactly **one** representation, so structural `==`/hash/display
  need no normalization. E.g. `Some(5)` is *always* the inline form, never the
  boxed form — the inline and boxed cases must hold **disjoint** payload classes.
- **Determinism — hashing must use LOGICAL tags, not Rust discriminants.** The
  hash (`vm_value.rs:246`) currently does `std::mem::discriminant(value).hash()`
  *before* the payload, and `Map` iteration order is FNV-1a over those hashes. Naively
  adding `OptionSomeScalar`/`VariantInline` gives them *new* discriminants, so
  `Some(5)` would hash differently than before → `Map` order shifts → parity breaks.
  **Hard requirement:** replace the discriminant-based prefix with an explicit
  **logical tag** so a value hashes identically regardless of representation —
  `OptionSomeScalar(5)` MUST hash exactly like the old `OptionSome(Box(Int(5)))`
  (same logical "Some" tag + same payload sequence), and inline variants MUST hash
  the **canonical name string** (`data.name`, as `Variant` does at `vm_value.rs:273`),
  never a tag index. The cheap tag-index (§4) is for dispatch/storage only; it must
  not leak into hash or equality. A `Map`-order regression test (build a map, snapshot
  iteration order) gates every slice.
- **`VmValue` size budget.** Today 16 bytes. Inlining grows it; every widening
  costs copy/cache traffic on *all* values (including scalar-heavy code). Treat
  `size_of::<VmValue>()` as a tracked metric with a hard cap (proposal: **≤ 24
  bytes**); anything that needs more must justify it against a scalar-loop
  regression check.
- **Dev startup stays instant.** No change may add eager work at program load. JIT
  remains lazy/tiered (cold interpreted, only hot compiled).
- **`panic = abort`, no_panic fuzzing, Miri scope** — see [[reg-vm-hardening]].
  Unboxing adds `unsafe` only if a slice proves it necessary; prefer safe enums.

## 4. The representation design (the crux — get this right before any slice)

**The recursion problem.** `OptionSome` / a struct field is itself a `VmValue`, so
a naively-inlined composite is infinitely sized — that is *why* there's a `Box`/
`Rc` today. Unboxing must break this without unbounded size.

**The lever: inline only the non-recursive / bounded cases.**
- **Scalars are free to inline** — `Int`/`Float`/`Bool`/`Char`/`Unit` are fixed,
  non-recursive, ≤ 8 bytes. An `Option`/1-field-variant *of a scalar* fits in the
  value with no heap cell.
- **Prerequisite (V2.0): a dynamic layout interner.** A `Struct`/`Variant` today is
  `Rc<VmStruct { name, layout, fields: Vec }>` (`vm_value.rs:98`), but **the layout
  is NOT interned** — `VmStruct::from_named` (`vm_value.rs:106`) allocates a **fresh
  `Rc<StructLayout>` on every construction**, and `StructLayout` holds only
  `field_names` (`vm_value.rs:80`), **not the name**. So the "keep the shared layout"
  idea below has an unmet prerequisite: a `TypeLayout { name, field_names, slot map }`
  that is **interned and shared**. It cannot be a lowering-time-only table, because
  **not all producers are lowered user types** — runtime/native/error paths build
  shapes whose `(name, fields)` are only known at runtime: `value_ok`/`value_err`
  (`value_access.rs:632/638`), **arbitrary** native-binding structs/variants in
  `vm_value_from_native_value` (`value_convert.rs:622`), and runtime errors like
  `json_error_value` (`runtime_values.rs:231`). So V2.0 is a **per-VM dynamic
  interner keyed by `(name, field_names)`**: `intern_layout(name, field_names) ->
  Rc<TypeLayout>` (first call allocates + caches, later calls return a refcount-bumped
  clone). Lowering pre-populates it for user types (so `MakeStruct`/`MakeVariant`
  fetch by a precomputed handle, no per-construction hashing); the runtime/native/error
  producers above call `intern_layout(...)` instead of `from_named`. This is *itself*
  an allocation win (one fewer `Rc::new` per construction for repeated shapes) and is
  what puts the name on the shared layout for the inline form to hash. **V2 cannot
  start until this lands.**
- **Composites must keep the shared (interned) layout; only the per-instance fields
  inline.** Once V2.0 interns the layout, the only *hot* per-instance allocation is
  the `Rc` box + `fields: Vec`. So an inline composite **must still carry the shared
  interned layout** (a cheap `Rc<TypeLayout>` clone = a refcount bump, no allocation)
  because slot-based field read/write
  (`GetFieldSlot`/`SetFieldSlot`), hash (`data.name` + fields, `vm_value.rs:273`),
  display, and native conversion **all need the real arity and field names**. A bare
  `[Scalar; K]` that drops the layout breaks all of those. Correct shape:
  `VariantInline { layout: Rc<TypeLayout>, fields: <inline> }` — the interned
  per-type layout (V2.0) shared, fields inline.
  Two sub-strategies for the *fields*, decided per-type by data:
  - **(a) Small-value optimization (SVO):** inline up to K **scalar** fields in a
    fixed buffer (spill to `Rc<VmStruct>` when > K or any field is non-scalar).
    **Start with K=1** — a single-scalar-field variant covers `Ok(x)`/`Err(x)`
    (Result) and most single-payload enums, and keeps `size_of::<VmValue>()`
    bounded (`Rc` ptr 8B + one `Scalar` 16B ≈ 24B). K≥2 multiplies the size cost on
    *every* value; only raise K if a struct sweep justifies it against the budget.
  - **(b) Arena/pool:** keep the 16-byte pointer but allocate the `VmStruct` cell
    from a **bump arena freed in bulk at scope exit** (escape-analysis scoped)
    instead of individual `malloc`/`free`. Kills the 40% churn *without* the size
    cost — but values stay heap, so it does **not** make them register-shaped
    (weaker JIT unlock).
  SVO unlocks Cranelift register-allocation; arena only helps the interpreter.
  Since the JIT-unlock is a primary goal, **prefer SVO (K=1) for single-field
  composites**, and use arena for the multi-field spill case.

**Canonical disjointness rule.** Define each unboxed variant so it can *never*
represent the same logical value as its boxed form:
- `OptionSomeScalar(Scalar)` holds *only* scalar payloads; the heap form holds
  *only* non-scalar (heap) payloads. A scalar `Some` is *always* inline. → an
  inline-Some and a heap-Some are never equal-but-different-rep.
- Define a single `Scalar` sub-enum (`Int|Float|Bool|Char|Unit`) reused across
  `OptionSomeScalar`, `VariantInline`, `StructInline` so the inline payload type is
  one well-tested thing.

**Enforce canonicality by RENAMING, not by exhaustiveness.** Adding a variant makes
non-exhaustive `match`es fail to compile, but **constructors keep compiling** — and
there are **~43 direct `OptionSome(Box::new(..))` producers** beyond `MakeSome`
(e.g. `reg_vm/value_access.rs:623`, `reg_vm/runtime_values.rs:107/119/192`,
`reg_vm/value_convert.rs`). A boxed scalar `Some` slipping through any of them
violates the canonical rule silently. **Required mechanism:**
- **Rename the heap form of EVERY type being unboxed** so raw construction can't
  compile: `OptionSome`→`OptionSomeHeap`, **and `Struct`→`StructHeap`,
  `Variant`→`VariantHeap`** (the V2 types have the same hole — direct producers
  construct `Struct`/`Variant` directly and would keep emitting non-canonical heap
  forms: the lowered `MakeStruct`/`MakeVariant` (`mod.rs:7795/7808`) **and** the
  non-lowered runtime/native/error paths `value_ok`/`value_err`
  (`value_access.rs:632`), `vm_value_from_native_value` (`value_convert.rs:622`),
  `json_error_value` (`runtime_values.rs:231`)). Renaming forces **every** producer
  to fail to compile and reroute through `VmValue::variant`/`structure` — which pull
  their layout from the V2.0 interner. (Alternative: make the variants private to the `vm_value`
  module so only the constructor can build them — equivalent guarantee.)
- Add **one canonical smart constructor per type** — `VmValue::some(value)`,
  `VmValue::variant(layout, fields)`, `VmValue::structure(layout, fields)` — that
  picks inline-vs-heap from the payload/arity. Every producer routes through it; no
  raw `*Heap`/`*Inline` construction outside the constructor. This makes the
  disjointness invariant *mechanically* enforced, not convention — and the
  constructor is the single place the canonical inline-vs-heap decision lives.

**Equality/hash/display must be representation-agnostic *by construction*** (via
the canonical constructors + logical hash tags, §3), not by special-casing.
Exhaustiveness covers the *match* sites (lean on it); the rename covers the
*construct* sites.

**Deliverable of this section before coding:** a written `VmValue` v2 enum
definition + a `size_of` budget table + the canonical-disjointness proof, reviewed
against every `is_hashable`/`PartialEq`/`Hash`/`display`/`deep_copy` site.

## 5. Phase V1 — Option scalar unboxing (first slice, highest ROI) — ✅ DONE

**Landed & soak-verified.** `Scalar { Int|Float|Bool|Char|Unit }` sub-enum added;
`OptionSome`→`OptionSomeHeap` (renamed in place, discriminant preserved) +
`OptionSomeScalar(Scalar)` at the enum end; single canonical `VmValue::some()`
constructor (scalar→inline, else→heap) with **all ~50 producers rerouted through it**
(the rename forced every raw producer to break). Hash kept **byte-identical** —
`OptionSomeScalar` emits the cached heap-`Some` discriminant prefix (via `OnceLock`,
no per-hash alloc) + the same payload sequence, proven by a hash-identity test and a
`Map`-iteration-order snapshot. **`size_of::<VmValue>()` unchanged (16→16)** — the
inline `Scalar` fit the existing layout. Canonical disjointness (scalar↔inline,
heap↔non-scalar) enforced by the constructor + tested. Compiled-Rust backend lowers
to real Rust `Option` (never references the variant), so output/Map-order parity holds.
**Gates:** `jit_acceptance` 8/8 (native), `differential` 31/31, V1 unit tests 5/5, and
the **full generative soak `ok` (31/31, 1142s, compiled backend included)**.
**Measured (V1.5):** `match_option_loop` **25.60→24.31 ms (−5.0%, non-overlapping
spreads)**; `option_result_chain` ~1% (its `Result` half stays boxed until V2 — as
predicted). The thesis holds: removing the `Box` per scalar `Some` is a real,
parity-safe interpreter win.

The smallest self-contained slice that proves the thesis. `Option` has dedicated
`OptionSome(Box<VmValue>)`/`OptionNone` variants (easy to change in isolation).
**Confirmed:** `Result` is **not** an Option-style value — it is a
`Variant(Rc<VmStruct>)` named `"Ok"`/`"Err"` (`mod.rs:8015`), so Result is unboxed
in **V2** (small-variant SVO), not here. The 651× `option_result_chain` kernel
exercises both, so its *full* win needs **V1 + V2**; V1 alone captures only its
`Option` half (`maybe_even`/`Option.map`/`and_then`/`unwrap_or`).

- [x] **V1.1 Representation + canonical constructor.** **Rename** `OptionSome` →
      `OptionSomeHeap` (forces all ~43 producers to break) and add
      `OptionSomeScalar(Scalar)`; `OptionNone` unchanged. Add the canonical
      `VmValue::some(value)` constructor (inline iff scalar, else heap) and **route
      every producer through it** — `value_access.rs:623`,
      `runtime_values.rs:107/119/192`, `value_convert.rs`, `MakeSome`, the intrinsics.
      No raw `OptionSomeScalar`/`OptionSomeHeap` construction outside the constructor.
- [x] **V1.2 Update every Option *match* site** — `MatchOption`, `UnwrapSome`,
      `is_hashable`, `PartialEq`, `Hash`, `display`, `deep_copy`, the map-key path,
      and the compiled-Rust + Cranelift lowerings. The exhaustiveness checker finds
      the matches; the V1.1 rename already forced the constructors.
- [x] **V1.2b Logical hash tag (§3).** `OptionSomeScalar(s)` and `OptionSomeHeap(b)`
      must hash with the **same logical "Some" tag** + identical payload sequence as
      the old `OptionSome`, so `Some(5)`'s hash — and every `Map`'s order — is
      unchanged. Add the `Map`-iteration-order snapshot test.
- [x] **V1.3 Canonical-disjointness audit** — prove inline-scalar-Some and
      boxed-Some can never both represent one value (scalar vs heap payload classes
      are disjoint).
- [x] **V1.4 Verify.** Fast gates green; then the **mandatory full soak** green.
- [x] **V1.5 Measure.** `option_result_chain` + `match_option_loop` before/after,
      `--mode vm-internal`, median + spread (§0.4 harness). Target: a real drop
      (the box-per-`Some` is a big chunk of those kernels). Record in a profile note.
- **Exit:** soak green + a measured, beyond-noise speedup on the Option cohort + no
  scalar-loop regression + `size_of::<VmValue>()` within budget.

## 6. Phase V2 — Variants (incl. Result) and small structs

- [x] **V2.0 Dynamic layout interner — DONE (soak green; variant_match_loop -17.8%, nested_struct_field -47.5%; TypeLayout{name,field_names} + thread-local interner + lowering-precomputed handle; size_of VmStruct 48->32, VmValue 16->16).** Today
      `VmStruct::from_named` (`vm_value.rs:106`) allocates a **fresh** `Rc<StructLayout>`
      per construction, and `StructLayout` (`vm_value.rs:80`) holds only `field_names`
      — **no name**. Introduce `TypeLayout { name, field_names, slots }` behind a
      **per-VM interner keyed by `(name, field_names)`**: `intern_layout(name,
      field_names) -> Rc<TypeLayout>`. Reroute **every** producer to it:
      `MakeStruct`/`MakeVariant` (`mod.rs:7795/7808`, via a lowering-precomputed
      handle — no per-construction hashing on the hot path), **and the non-lowered
      runtime/native/error producers** `value_ok`/`value_err` (`value_access.rs:632`),
      `vm_value_from_native_value` (`value_convert.rs:622`, arbitrary native shapes),
      `json_error_value` (`runtime_values.rs:231`). The interner is the single layout
      source for both compile-time and runtime-determined shapes. Win on its own (one
      fewer `Rc::new` for repeated shapes) **and** it puts the name on the shared
      layout so the inline form (V2.1) can hash by it. Soak-verified on its own.
      (VM is single-threaded → an `Rc`-based `HashMap` interner, no `Arc`/locking.)
- [✗] **V2.1 Small-variant SVO — IMPLEMENTED, MEASURED, REJECTED (net-negative).**
      Built fully (`VariantInline { layout: Rc<TypeLayout>, field: Scalar }`,
      `Variant`→`VariantHeap`, canonical `VmValue::variant()` with re-canonicalizing
      field-writes, all producers + native conversions rerouted) and it is
      **parity-clean: full soak green (31/31, 1188s)**. But a clean same-machine
      before/after (vs V1+V2.0) shows it is **net-negative across the board**:
      `pure_loop_sum` **+29.6%**, `bool_logic_loop` **+29.1%** (scalar loops),
      `variant_match_loop` **+6.4%**, `option_result_chain` **+61.9%** — *worse on the
      very kernels it targeted*. **Root cause:** a variant needs a layout pointer
      *plus* the field, so `VariantInline` = `Rc`(8B)+`Scalar`(16B) = 24B → `VmValue`
      grew **16→24**, and a 50%-larger value taxes *every* interpreter copy/move (the
      ~29% scalar hit). And because **V2.0 already eliminated the layout allocation**
      (the big −47%/−18% win), inlining the field only saves ~1–2 mallocs per
      `Ok(int)` — far less than the size-tax + per-op `Scalar`-conversion/branching
      overhead it adds. Per the §3 size budget + the V1-proof-gate rule (≥10% scalar
      regression ⇒ do not ship), **not applied to main.** A 16B-preserving variant
      (intern by `(name, field_names, field_scalar_kind)` so the layout carries the
      type and the inline field is raw 8 bytes) would remove the *size* tax but not
      the per-op overhead, and after V2.0 the remaining payoff is small — **not worth
      it.** Lesson: **inline-SVO pays for `Option` (V1, fits 16B, no layout pointer)
      but NOT for composites** (need a layout pointer); the composite win was V2.0.
- [✗] **V2.2 Small-struct SVO — SKIPPED (same problem, worse).** Structs need the
      same layout pointer *and* multiple fields → an even larger `VmValue` and a bigger
      size tax, for a payoff V2.0 already captured (`nested_struct_field` −47.5% from
      the interner alone). Inline-SVO for structs is net-negative a fortiori. Not pursued.
- **Remaining composite-allocation lever (if revisited): arena/pool, not inline-SVO.**
      The only way to cut the residual `Rc<VmStruct>` malloc/free without the
      `VmValue` size tax is the §4 fallback (b): allocate the cell from a bump
      arena / freelist (keep `VmValue` at 16B), recycling instead of eliminating. But
      V2.0 already captured the largest composite win, so this is lower-priority and
      uncertain. **Conclusion of V2:** the layout interner (V2.0) was the real
      composite win; field inlining (V2.1/V2.2) is refuted by measurement.

## 7. Phase V3 — Collections COW — NOT WARRANTED (data-gated, gate failed)

- [x] **V3.1 Quantified — gate FAILED, V3 not pursued.** Callgrind on the
      collection kernels confirms they are heavily **allocator-bound** —
      `map_insert_lookup` **~48%** of retired Ir in `malloc`/`free`,
      `list_index_scan` **~45%** — but **`deep_copy_value` does NOT appear in the hot
      path at all.** The allocation is per-operation / collection-internal (HashMap
      node allocation as the map grows, intermediate construction), **not**
      whole-collection value-semantic copies. So COW/`imbl` (which only avoids
      *copies*) would not touch the actual cost. Per the plan's own data-gate ("do
      if data says"), **V3 is not warranted** and was not implemented. (The real
      collection-allocation lever would be pooling the HashMap/Vec backing storage —
      a different, larger project, not COW.)
- [ ] ~~V3.2 / V3.3~~ — moot (gate failed).

## 8. Phase V4 — Teach Cranelift to consume unboxed values (the JIT unlock)

This is where the representation work pays the second dividend. Builds on V1–V2.
> V4.3 (escape analysis) and §9's OSR are the entry point to the larger
> [`vm-optimizing-jit-plan.md`](./vm-optimizing-jit-plan.md) — a C2-class
> speculating JIT (profiling → monomorphic inlining → escape analysis + scalar
> replacement → deopt). V4 here is the minimal, non-speculating slice; J3/J5 there
> are the full treatment, gated on the precise-deopt foundation (J0).
- [ ] **V4.1 Register layout** for unboxed values: an unboxed `Option<scalar>` /
      small variant / small struct becomes a small fixed set of machine registers
      (tag reg + payload reg(s)) in the Cranelift IR — no host call, no heap.
- [ ] **V4.2 Make/Match/Unwrap in registers** — lower `MakeSome`/`MatchOption`/
      small `MakeStruct`/`GetFieldSlot` to register ops when the value is unboxed,
      instead of the host-helper boundary. (Float-read ABI from perf-plan §3.2 is the
      template; this generalizes it to constructed-in-register values.)
- [ ] **V4.3 Escape analysis (ours, at RegInstr level).** Mark a constructed value
      *non-escaping* when it is not stored into a heap container, not returned, not
      captured, not compared-by-identity (trivially true — value-semantic). A
      non-escaping unboxed value is **scalar-replaced**: it lives only in registers
      and is never allocated — the C2 trick. This is the step that turns a hot
      alloc loop into an allocation-free compiled loop.
- [ ] **V4.4 Cost guard** (fixes the perf-plan regression finding): do **not**
      dispatch native for a function whose eligible body is below an
      instruction-count threshold, so tiny leaf helpers stop losing to call-boundary
      overhead. Re-measure `closure_alloc`/`option_result_chain` native paths.
- [ ] **V4.5** differential per new opcode family (perf-plan §3 strict rule) using the
      **existing native force-deopt backend** (the §7.2 re-run-from-top fallback) — V4
      is the *minimal, non-speculating* slice, so it does **not** need the optimizing
      JIT's J0 safepoint-deopt machinery; the existing fallback suffices because these
      reads stay side-effect-free. (The full speculative escape-analysis/scalar-
      replacement version is J3 of `vm-optimizing-jit-plan.md`, which *does* require
      J0.) Then soak. **Exit:** hot alloc-bound kernels improve under `jit-native`
      *beyond* the interpreter win, parity green on success and bail paths.

## 9. Phase V5 — OSR + tiering policy (make the JIT actually fire on dev workloads)

From the perf-plan §3.4 finding: a once-called function with a hot inner loop never
tiers up (call count tops at 1). That class is exactly what **OSR** fixes.
- [ ] **V5.1 Spec amendment FIRST** (perf-plan §3.3 blocker): Exec-Spec §7 currently
      says OSR is "not applicable." Amend it with mid-loop entry/exit state mapping +
      the parity argument *before* any code (implementing against the current spec
      would be unsound).
- [ ] **V5.2 OSR-entry** for hot loops (JSC/HotSpot model): transfer a running
      interpreted loop into compiled code mid-execution. Spike the win on a
      long single-call loop kernel.
- [ ] **V5.3 Tiering policy** tuned (`RSS_JIT_TIER_THRESHOLD` already exists) so cold
      code stays interpreted (instant dev startup) and only proven-hot loops compile.
- **Exit:** the once-called-hot-loop class tiers up; dev startup unchanged; soak green.

## 10. Verification strategy (per slice, non-negotiable)

1. **Exhaustiveness as a safety net** — adding a `VmValue` variant makes every
   non-exhaustive `match` a compile error; that *is* the checklist of sites to update.
2. **Fast inner loop** — `cargo test -p rsscript --test runtime jit_acceptance`
   (default + `--features native-jit`) and `--test differential`.
3. **Slice-exit gate — the full generative soak** (command in §3, ~20 min). This is
   the only gate that exercises enough random value shapes to catch a
   representation/equality bug. **No slice merges without a green soak.**
4. **Performance gate** — §0.4 harness: median + spread before/after on the touched
   cohort, beyond-noise improvement, and a **scalar-loop non-regression** check
   (since `VmValue` may have grown) + a `size_of::<VmValue>()` assertion test.
5. **Disk** — the soak regenerates large compiled-test caches; clear
   `target/rsscript-generated-test` between runs (it reached ~97G this session).

## 11. Sequencing & exit criteria

```
V1 Option scalar unbox ─► V2 variants (incl. Result) + small structs ─► (V3 COW, if data)
                                          │
                                          ▼
                         V4 Cranelift consumes unboxed + escape analysis
                                          │
                                          ▼
                         V5 OSR + tiering (needs spec amendment first)
```
- **V1 is the proof-of-thesis** and must land + show a real Option-cohort speedup
  before V2 is funded. Measure on `match_option_loop` (pure-Option, the clean
  signal) — `option_result_chain` only *partially* improves under V1 because its
  `Result` half is still boxed until V2. If V1 does *not* show a clear beyond-noise
  win on `match_option_loop`, re-examine the thesis before generalizing.
- **V1–V3 are interpreter-only** wins (dev-wide, no JIT) — ship them first; they
  stand alone even if V4/V5 never happen.
- **V4 requires V1–V2** (nothing to register-allocate until values are unboxed).
- **V5 requires the spec amendment** and is independent of V4 (can proceed in
  parallel once V1–V2 land).
- Every slice exits on: green soak + measured beyond-noise win + size budget held.

## 12. Risks

- **`VmValue` bloat.** Inlining grows the value; a 48-byte value would regress
  scalar-heavy code via copy/cache cost. Mitigation: hard size cap (§3), scalar-loop
  regression gate, prefer SVO with small K, fall back to arena for spill.
- **Representation aliasing → parity bug.** Two reps of one value breaking `==`/hash
  is the classic hazard. Mitigation: the canonical-disjointness rule (§4) + the soak.
- **`unsafe`/Miri.** SVO buffers may tempt `unsafe`; prefer safe enums, and if
  `unsafe` is unavoidable, keep it in one audited module under Miri.
- **JIT parity surface (V4).** Each new in-register opcode multiplies the parity
  surface — strict force-deopt differential per family (perf-plan §3 rule).
- **Effort.** This is weeks, not a session — V1 alone touches every Option site. It
  is staged precisely so each slice is independently shippable and soak-verified.

## 13. Open questions

- K for SVO (inline field count) — **start K=1** (covers Result `Ok`/`Err` and
  single-payload enums at ~24B); raise only if a struct-kernel sweep beats the
  `size_of::<VmValue>()` budget cost. Decide from V1 size data.
- Layout-id index vs `Rc<TypeLayout>` clone for inline composites (after V2.0
  interning) — the clone is a refcount bump (no alloc) and is simplest; a `u32`
  layout-id into a side table is smaller but adds an indirection. Measure both
  against the size budget.
- ~~Is `Result` a `Variant` or dedicated variants?~~ **Resolved:** `Result` is a
  `Variant(Rc<VmStruct>)` named `"Ok"`/`"Err"` (`mod.rs:8015`) — so it unboxes in V2,
  and `option_result_chain`'s full win needs V1+V2.
- Does the compiled-Rust backend's value model constrain the canonical rep (it must
  still produce identical observable output)? Audit `rust_lower/` for Option/struct.
- Arena vs SVO for the spill case — decide from V1/V2 size-vs-alloc data.
- Can `Native`/`Managed` ever be unboxed, or are their identity/interior-mutability
  semantics load-bearing? (Out of scope until audited.)
