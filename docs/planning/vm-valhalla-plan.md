# Valhalla plan — type-specialized layout (flat typed containers)

Adopt Java-Valhalla-style **type-specialized layout** for RSScript's reg-VM: store
values laid out by their **static type** instead of as uniform `VmValue` boxes —
**without** breaking the §2 parity invariant and **without** the `VmValue` size tax
that sank the uniform-slot inlining (value-rep plan V2.1).

Companion to [`vm-value-rep-plan.md`](./vm-value-rep-plan.md) (which validated V1
Option-unbox + V2.0 layout-interner, and **refuted** uniform-slot composite
inlining). This plan pursues the *correct* Valhalla shape that V2.1 got wrong.

Status: **TV1+TV2 SHIPPED (typed lists + native direct reads), soak-verified.**
Created 2026-06-20.

**Validated result (TV1 + TV2 + TV2.1 + TV2.2, all landed on `main`):**
- **Native list-read loops ~3.7–3.9× faster** — `native_read_heap` (List\<Int>)
  9.9→2.5 ms, `native_read_heap_float` (List\<Float>) 6.9→1.9 ms; `nat/reg` 0.07→0.02
  (near scalar-native). The per-element host-call boundary (§7.1's 13× read-heap
  penalty) is eliminated by reading the flat `Vec<i64>`/`Vec<f64>` directly in
  registers.
- **Interpreter: neutral.** Non-list cohorts +1.1–1.6% (within noise); list cohorts
  +1.8–2.7% (< 5%).
- **`VmValue` stays 16 bytes** (the V2.1 size trap avoided — typed containers live
  behind a pointer).
- Parity: full generative soak green (33/33, compiled backend incl.), force-deopt
  differential on the direct-read path (success + OOB-bail), jit_acceptance, vm-jit.

**How it got there (the slices):**
- **TV1** — `TypedVec { Boxed | Ints | Floats }` behind the `List` `Rc`; kind-agnostic
  accessor API; dynamic kind selection at construction (`TypedVec::from_values`, no IR change);
  producer audit; per-kind `mem_budget`. *Interpreter-only; was net-negative alone
  (the flat buffer can't be cashed by the interpreter, which re-materializes).*
- **TV2** — native tier reads the flat array directly (`*const f64/i64` + len,
  bounds-checked, IR_VERSION→4) under a **pinned `Ref` borrow** for the call's
  duration (sound: native-eligible = side-effect-free). The win.
- **TV2.1** — typed fast paths + amortized (capacity-growth) `mem_budget` accounting,
  removing the per-element list-opcode tax.
- **TV2.2** — **`#[inline(never)]` extraction** of the 8 enlarged list opcode bodies
  out of the `#[inline(always)] try_exec_pure` hot loop (perf-plan §1.3 hot/cold
  split). This recovered a **uniform ~10% interpreter regression** that TV1+TV2's
  inline bloat had caused across *all* opcodes (incl. non-list) — confirmed by the
  non-list controls returning from +10% to +1%.

## 1. The thesis, and why V2.1 failed
V2.1 tried to inline a composite into the **shared** `VmValue` tagged enum →
`VmValue` grew 16→24 → **every** value copy (even `Int` loops) paid a ~29% tax,
which dwarfed the savings. That is the *opposite* of Valhalla: Valhalla never taxes
`int` to support `Point`, because **it has no uniform value box** — layout is
specialized per static type.

**The fix: flatten typed CONTAINERS (behind a pointer), not the uniform value.**
- A `List<Float>` as `Rc<RefCell<Vec<f64>>>` keeps the `VmValue::List` variant at
  one pointer → **`VmValue` stays 16B, zero size tax** — while the *elements* are
  flat (no per-element box, ½ the buffer, cache-friendly, and a real `f64[]` the
  JIT can index directly without the §7.1 per-element host-call boundary).
- This is Valhalla's `Point[]`-is-flat idea applied where it fits the uniform model.

**Soundness — typed is a NON-canonical optimization, not a canonical invariant.**
(Revised after review.) Do **not** require "a scalar list is *always* the typed
kind" — list values are produced in dozens of places (native conversion, `Args.all`,
`Map.keys`, string `split`/`lines`, JSON arrays, tensor dims, sorted-map entries,
stream/channel helpers, `map`/`filter`/`slice`), and not all of them cheaply know an
element kind. Forcing every producer to specialize is brittle. Instead:
- `Boxed`, `Ints`, `Floats` are **interchangeable representations of the same
  logical list**; **eq / hash / display / native-conversion are kind-agnostic**
  (they materialize logical `VmValue`s), so a `Floats` list and a `Boxed([Float,…])`
  list are **observably identical** — parity holds no matter which kind a producer
  emits.
- A producer emits the typed kind **only when it cheaply knows the element kind**
  (the perf win); otherwise it emits `Boxed` (always correct). No canonical-uniqueness
  obligation, no aliasing hazard.
- RSScript's static typing still *guarantees homogeneity* (a `List<Float>` is all
  floats), so specializing is always safe where applied — it's just not *mandatory*.

## 2. Hard constraints (do not regress) — same as value-rep §3
- **§2 parity sacrosanct**; **full generative soak mandatory per slice** (the gate).
- **No `VmValue` size growth.** Typed containers stay behind a pointer →
  `size_of::<VmValue>()` unchanged (assert ≤ 16). This is the whole point.
- **Kind-agnostic observation (replaces the canonical rule):** eq / hash / display /
  native-conversion / `deep_copy` must produce **byte-identical** results regardless
  of `TypedVec` kind (they iterate logical `VmValue`s). A `Boxed` and a typed list of
  the same values are interchangeable. No canonical-uniqueness requirement.
- **Determinism — narrowed (Finding 5).** Map-key **hash** parity matters only for
  **hashable** lists, i.e. `List<Int>` (`is_hashable` is true only when every element
  is hashable; **`Float` is NOT hashable**, so `List<Float>` can never be a map key).
  So: `Ints` lists need full hash+eq+display+native parity; `Floats` lists need
  eq+display+native parity (no map-key hash path exists for them). Keep the
  `Map`-order snapshot test, keyed by `List<Int>`.
- **Producer audit (Finding 2).** Every `VmValue::List(Rc::new(RefCell::new(...)))`
  construction site must be visited and made **intentional**: either it knows an
  element kind and emits the typed `TypedVec`, or it explicitly emits `Boxed`. None
  may be left implicitly boxed-by-accident. (Correctness doesn't require specializing
  — kind-agnostic observation covers it — but the audit ensures the perf-relevant
  producers are specialized and nothing is overlooked.)
- **`mem_budget` accounting per kind (Finding 4).** List accounting today charges a
  fixed `LIST_ELEM_BYTES` (≈ `size_of::<VmValue>()`) per element (`mod.rs:8141`).
  A `TypedVec` must charge per kind: **8 bytes/elem for `Ints`/`Floats`** (+ container
  overhead), the `VmValue` cost for `Boxed`. Update every list-growth accounting site
  so the sandbox memory ceiling stays correct and not misleading.
- Worktree + sub-agent + Docker; no hack; reject net-negative with data (V2.1 rule).

## 3. Staged slices (each soak-gated, measured, independently shippable)
> **Status: TV1, TV2, TV2.1, TV2.2 all SHIPPED** (see top). The next milestone is
> **typed-list loop optimization** (hoist the typed-list handle/len once per loop;
> eliminate repeated bounds checks when the loop shape proves `i < len`; recognize
> sum/scan/fold loops over `List<Int>`/`List<Float>`; optionally inline tiny scalar
> closures). **TV3/TV4 are data-gated and NOT next by default** — typed lists just
> solved the JIT's biggest real-world gap; milk that path first. Only start TV3
> (struct fields) when a benchmark shows struct fields are the dominant remaining cost.
- **[x] TV1 — typed scalar list storage (interpreter). SHIPPED.** (Was the proof slice.)
  `VmValue::List(Rc<RefCell<TypedVec>>)` where `TypedVec = Boxed(Vec<VmValue>) |
  Ints(Vec<i64>) | Floats(Vec<f64>)`. **As shipped, the kind is chosen *dynamically*
  from the element values via `TypedVec::from_values`, not from static bytecode
  metadata** — `MakeList { dst, items }` is unchanged, and a non-empty homogeneous
  literal specializes correctly with no IR change (Finding 1's `elem_kind` field was
  not needed for the win). The one case `from_values` can't see is an *empty*
  `List<Float>`, which starts `Boxed(vec![])` and specializes on its first scalar
  `push` — a parity-neutral late-bind. A static `ListNew { elem_kind }` to start empty
  typed lists pre-specialized remains an **optional** future refinement, not shipped.
  All other list ops + eq/hash/display/deep_copy/native-conversion go through
  the kind-agnostic accessor API. **Producer audit** every `List(Rc::new(...))` site.
  **Interpreter-only**; measure a `List<Float>`/`List<Int>` sum/scan win.
- **[x] TV2 — JIT reads flat arrays directly. SHIPPED** (with TV2.1 fast-paths +
  TV2.2 hot/cold split — the ~3.7–3.9× native win; design below as built).
  Extend the native tier so `ListGet`/`ListLen` on a `Floats`/`Ints`
  list take the raw `*const f64`/`*const i64` + len and index in-register —
  **eliminating the per-element host-call boundary** (the §7.1 13× read-heap penalty).
  **Soundness mechanism (must be specified before coding):** reading a raw pointer out
  of `Rc<RefCell<Vec<_>>>` is sound only if the backing vector cannot reallocate or
  mutate during the native call. Native-eligible functions are side-effect-free
  (§7.2), so the protocol is: **pin a `Ref` (shared borrow) of the `RefCell` for the
  duration of native execution** (or snapshot the pointer+len under that borrow), so
  no `borrow_mut`/realloc can occur; the handle table already hands native a stable
  reference. State and test this before TV2 lands. Force-deopt differential per shape.
- **[~] typed-list loop optimization — INVESTIGATED; core already worked.** A CLIF
  audit of the native sum/read loops (`native_sum_loop.rss`, `native_read_heap.rss`)
  found:
  - **Recognize sum/scan/fold over `List<Int>`/`List<Float>` — already native-eligible
    and ~128× over the interpreter** (a pure `while i < List.len(xs)` sum in a leaf
    function: 9.4 ms native vs 1202 ms interpreter). No new recognition pass needed.
  - **Fixed a real eligibility bug (shipped):** `DeepCopy` of an *untyped* register
    (e.g. an unused/under-typed parameter) wrongly rejected the **whole** function for
    native translation (`ty[reg]?` in the lowerer). A `DeepCopy` in a pure leaf
    function is always a no-op, so it now lowers to `Nop` unconditionally — unblocking
    otherwise-eligible numeric loops that carry a config/unused param.
  - **Hoist len once per loop — tried, reverted (perf-neutral).** A deterministic
    entry-block hoist of each flat param's `len` (Cranelift's GVN won't lift the
    `readonly` load across the loop back-edge) removed one load/iter in the CLIF but
    measured within noise on both the read and pure-sum kernels. The `len` reload was
    never the bottleneck, so it was not shipped (measure-and-reject).
  - **Bounds-check elimination — not pursued.** Limited upside (see next bullet) and a
    high safety bar (a mis-proof is an out-of-bounds native read = UB).
- **[ ] (NEXT LEVER) per-iteration overflow-checked Int arithmetic — DATA-GATED.** The
  CLIF audit showed the dominant remaining gap to compiled Rust (~4.4× on int loops) is
  **not** list access: every `total += x` / `i += 1` lowers to `sadd_overflow` + a bail
  branch (RSS Int overflow semantics defer to the interpreter), which also blocks
  auto-vectorization. Closing it is a *language-semantics* effort (opt-in wrapping `Int`,
  or a "provably non-overflowing" range analysis that drops the checks), separate from
  typed-list work. Documented here as the real next performance lever; not started.
- **[ ] TV3 — typed struct fields. DATA-GATED, not next.** Flatten a struct's scalar
  fields in `VmStruct` — **only** when a benchmark shows struct fields are the
  dominant remaining cost. Do not start by default; typed lists just solved the
  JIT's biggest real-world gap — milk that path first.
- **[ ] TV4 — typed `Map`/`Deque` element storage. DATA-GATED**, same rule.

## 4. TV1 spec (the first slice)
1. `TypedVec` enum (heap, behind the existing `List` `Rc`): `Boxed(Vec<VmValue>)`,
   `Ints(Vec<i64>)`, `Floats(Vec<f64>)`. `VmValue` unchanged (still `List(Rc<RefCell<…>>)`)
   — assert `size_of::<VmValue>() <= 16`.
2. **Kind-agnostic accessor API** on `TypedVec` so the ~85 sites work through it:
   `len`, `get(i) -> Option<VmValue>` (materializes), `set`/`push`/`pop`/`insert`/
   `remove`/`clear`/`extend`, `iter()`/`to_vec()` (materialize logical `VmValue`s),
   `from_values(Vec<VmValue>, hint)`. eq/hash/display/`deep_copy`/native-conversion go
   through these → **kind-agnostic, byte-identical to the old boxed list** (this is the
   parity guarantee; there is **no** canonical-uniqueness requirement — `Boxed` and
   typed reps of the same list are interchangeable).
   - **Kind-mismatch rule (mutation helpers must be TOTAL — never panic):** when a
     value pushed/set/extended doesn't match the `TypedVec` kind (e.g. a non-`Int`
     into `Ints`), behave by *context*, with **two distinct helpers**:
     - **construction / conversion / producer paths** (`from_values`, list-building
       intrinsics) → **promote the whole `TypedVec` to `Boxed`** and store the value
       (the kind hint was optimistic; degrade gracefully, never lose data or error);
     - **checked VM mutation opcodes** (`ListPush`/`ListSet`/`ListInsert` from
       `RegInstr`, where the type checker *guarantees* the element type) → return a
       **runtime `EvalError`** on mismatch. A typed `List<Int>` receiving a non-`Int`
       there means the checker or a native bridge violated its contract — surface it
       loudly, don't silently corrupt. (In practice this path never fires; it's the
       defensive total-function tail.)
3. **Construction (Finding 1) — shipped as dynamic `from_values`, not static metadata.**
   The shipped path leaves `MakeList { dst, items }` untouched and picks the kind from
   the element values at runtime via `TypedVec::from_values` (a non-empty homogeneous
   `Int`/`Float` literal specializes; anything mixed or heap stays `Boxed`). This needed
   no IR change and captured the literal-construction win directly. The only gap is an
   *empty* typed list (`from_values` has nothing to inspect), which starts `Boxed(vec![])`
   and late-binds its kind on the first scalar `push` — parity-neutral. Threading a static
   `elem_kind` from the lowerer into `MakeList`/`ListNew` (so empty `List<Float>` starts
   `Floats(vec![])` pre-specialized) is an **optional** future refinement, deferred until a
   benchmark shows empty-then-fill lists matter. `elem_kind ∈ {Boxed, Ints, Floats}` would
   specialize when known; `Boxed` is always the correct fallback.
4. **Producer audit (Finding 2).** Visit **every** `VmValue::List(Rc::new(...))` site
   (native conversion, `Args.all`, `Map.keys`, string `split`/`lines`, JSON arrays,
   tensor dims, sorted-map entries, stream/channel helpers, `map`/`filter`/`slice`/
   `partition`, …) and make each intentional: emit the typed kind where the element
   kind is cheaply known, else `Boxed`. None left boxed-by-accident.
5. **Determinism (Finding 5, narrowed).** `Ints` lists are hashable → full
   hash+eq+display+native parity (keep a `Map`-order snapshot test keyed by
   `List<Int>`). `Floats` lists are **not** hashable (`Float` isn't) → eq+display+native
   parity only; no map-key path exists for them.
6. **`mem_budget` (Finding 4).** Update list-growth accounting (`mod.rs:8141`,
   `LIST_ELEM_BYTES`) to charge **8 B/elem for `Ints`/`Floats`**, the `VmValue` cost
   for `Boxed`, + container overhead. Verify with a `mem_budget`-enabled test.
7. **Verify:** fast gates + **full soak**; the `size_of` assert; the parity unit tests
   (typed≡boxed eq/hash/display, `Map`-order with `List<Int>` keys, construction kind);
   measure a `List<Float>` and `List<Int>` sum/scan kernel (`--mode vm-internal`)
   before/after + a scalar-loop non-regression check. **Reject if net-negative (V2.1 rule).**

## 4a. Implementation order (discipline — do this, it's a broad mechanical slice)
1. **Centralize FIRST.** Land `TypedVec` + the full kind-agnostic accessor API +
   eq/hash/display/deep_copy routed through it, with **only the `Boxed` kind** in use
   (every producer builds `Boxed`, every site reads via the accessor). This is a pure
   refactor — observable behavior identical — and should pass the soak on its own. It
   shrinks the ~85 raw-`Vec` sites to the accessor surface.
2. **Then introduce `Ints`/`Floats`** and the `elem_kind` construction metadata.
3. **Then migrate producers one by one** (the audit), re-running the fast gates after
   each, the soak before shipping. Don't flip everything at once.

## 5. Risks
- **Op surface:** lists have many ops; all must handle the kinds (exhaustiveness
  helps). Bounded but sizeable — the proof slice's cost.
- **Generic `List<T>`:** when the concrete element type isn't known at lowering, fall
  back to `Boxed` (partial specialization — the concrete-type hot paths still win).
- **The big payoff is TV2 (JIT direct-read);** TV1 alone is a moderate interpreter
  win — but TV1 is the prerequisite and the proof that typed containers are
  parity-safe and size-neutral. Gate TV2 on TV1 landing.
- **Honesty:** if TV1 doesn't win in the interpreter, the typed-container thesis is
  weaker than hoped — report and reassess before TV2 (V1-proof-gate discipline).
