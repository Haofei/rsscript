# Phase 4 — Value representation profile (profile-4.1)

Scope: decide whether Phase 4 (value/`Rc` representation) is justified by the
0.3 profile, and if so implement the single safest parity-neutral reduction in
the reg-VM hot path. Parity is sacrosanct (§2): bit-for-bit identical output
across all tiers.

Environment: Docker `dev` container, prebuilt release binary `/work/target/release/rss`.

---

## 4.1 Alloc/`Rc` traffic quantification

Method: one foreground callgrind run (`--tool=callgrind --cache-sim=no`) over

```
rss bench --mode vm-internal benchmarks/micro/option_result_chain.rss 30000 \
    --vm reg --iterations 1 --warmup 0
```

then `callgrind_annotate --inclusive=no --threshold=95`. We read exclusive Ir
(instructions retired) per function to find where the time goes. The
option/result cohort was the worst in the 0.3 profile (652x vs Rust) and is
allocation-dominated by construction (`OptionSome(Box<VmValue>)` boxes per
`Some`).

Real output (callgrind 3.19.0, `cache-sim=no`, PROGRAM TOTALS = 658,144,202 Ir;
the bench reported `mean_ms=103936.926` — wall time is dominated by valgrind
instrumentation, so we read Ir, not ms). Top exclusive-Ir functions:

```
144,034,477 (21.88%)  RegVm::drive'2
 86,935,598 (13.21%)  malloc.c:_int_free                 [libc]
 64,743,276 ( 9.84%)  malloc'2                           [libc]
 38,322,154 ( 5.82%)  free'2                             [libc]
 26,310,334 ( 4.00%)  core::ptr::drop_in_place<VmValue>'2
 25,092,852 ( 3.81%)  malloc.c:_int_malloc'2             [libc]
 21,166,602 ( 3.22%)  lexer::lex'2                       (one-shot compile floor)
 13,379,176 ( 2.03%)  malloc.c:_int_free'2               [libc]
 12,142,193 ( 1.84%)  malloc.c:_int_malloc               [libc]
 11,969,763 ( 1.82%)  eval_numeric_compare'2
 11,700,000 ( 1.78%)  RegVm::call_closure_one
 11,295,308 ( 1.72%)  <VmValue as Clone>::clone'2
 11,005,080 ( 1.67%)  RawVecInner::finish_grow
 10,439,931 ( 1.59%)  VmStruct::from_named'2
 10,170,724 ( 1.55%)  alloc::rc::Rc<T,A>::drop_slow'2
```

**Finding — allocation/`Rc` traffic dominates, exactly as the 0.3 profile predicted.**
Summing the allocator family (libc `malloc`/`free`/`_int_malloc`/`_int_free`/
`malloc_consolidate`/`unlink_chunk`/arena-free): roughly
`13.21 + 9.84 + 5.82 + 3.81 + 2.03 + 1.84 + 1.41 + 1.10 + 0.77 + 0.72 ≈ 40%` of
all retired instructions are spent **inside the C allocator**. Add the
Rust-side drop/clone machinery feeding it — `drop_in_place<VmValue>` (4.00%),
`Rc::drop_slow` (1.55%), `<VmValue as Clone>::clone` (1.72%) — and well over
**45%** of the run is value alloc / dealloc / refcount churn, *not* dispatch
(`drive` is 21.88% but a large fraction of *that* is the inlined per-op
clone+drop of `VmValue` operands). `VmStruct::from_named` (1.59%) and the
`Ok`/`Err`/`Some` intrinsics (`exec_option_intrinsics` 1.24%,
`exec_result_intrinsics` 1.03%, `result_variant_payload` 0.89%+0.62%) are the
producers of that allocation: each `Some`/`Ok`/`Err`/struct construction boxes
or `Rc`-allocates, and each match/unwrap clones the inner value and then drops
the boxed/Rc wrapper.

This justifies Phase 4 (the 0.3 "skip unless justified" gate is cleared) and
points the fix squarely at **reducing per-op `VmValue` clone+drop churn in the
hot path** (4.3), not at value width (4.2, skipped).

---

## 4.2 NaN-boxing decision — SKIP

**Decision: do NOT pursue NaN-boxing.**

Reasoning:

1. **Width is already fine.** `VmValue` is a 16-byte tagged enum
   (`crates/rsscript/src/vm_value.rs:52`). NaN-boxing's main benefit is squeezing
   a value into 8 bytes; at 16 bytes the per-value width is not the bottleneck on
   64-bit, and the 0.3 I-cache check showed the pressure is data-side
   (boxed `VmValue` / `Rc`), not code/tag-match.

2. **It does not address the real cost.** The 0.3 data shows the dominant cost is
   *allocation and `Rc` refcount traffic* (`OptionSome` `Box` alloc/free, `Rc`
   clone/drop), not tag matching. NaN-boxing reduces tag-match / value width; it
   does nothing for the heap allocation of `Some`, list/map/struct `Rc` bodies,
   or refcount inc/dec. So it would not move the worst cohorts.

3. **Large parity / UB risk.** Parity here is bit-for-bit:
   - Float bit-patterns: NaN-boxing co-opts the NaN payload space of `f64`;
     storing pointers in NaN payloads makes signaling/quiet NaN handling and
     canonicalization observable. RSScript has **deterministic float formatting**
     as an observable contract — any change to how floats round-trip risks a
     parity break that the differential gate would (rightly) reject.
   - Pointer tagging is target/word-size dependent and leans on assumptions about
     the high bits of host pointers — fragile under Miri and across platforms,
     conflicting with the no_panic / Miri hardening scope.
   - `Map` iteration order is observable and deterministic; any value-rep churn
     that perturbs hashing/equality risks it.

   The benefit (none for the actual bottleneck) does not justify the risk.

**Conclusion:** SKIP NaN-boxing. Pursue the lower-risk, higher-leverage path of
cutting `Rc`/`Box` clone+drop churn in the interpreter hot path (4.3).

---

## 4.3 Chosen optimization — evaluated, DEFERRED (with exact follow-up design)

**Outcome: no code change shipped this step.** Every move-vs-clone candidate the
4.1 profile points at depends on per-register liveness that the reg-VM does not
currently track, so taking the source register (instead of cloning it) is not
*provably* parity-neutral and would risk the §2 bit-for-bit invariant. Per the
task's explicit allowance, this is a documented deferral with the exact design,
not a forced unsafe change.

### What I evaluated (and why each is unsafe *today*)

The profile's allocation cost is produced by `Some`/`Ok`/`Err`/struct
construction and consumed by the matching unwrap/intrinsic, which **clone the
inner value out of the boxed/`Rc` wrapper and then drop the wrapper**. A move
would remove a clone+drop pair — *but only if the source register is dead after
the op*. Concretely:

1. **`MakeSome` (`reg_vm/mod.rs:7771`)** — `let value = self.reg(base+*value).clone();`
   then boxes it. The source `value` register is a general operand; the reg
   allocator here is **not SSA** and carries **no liveness/last-use info**
   (confirmed: there is no `last_use`/`dead_after`/consume flag on `RegInstr`,
   and `reg`/`set_reg`/`take_reg` at `mod.rs:8070-8105` are the only register
   API). If that register still holds a live variable, `take_reg` (which leaves
   `Unit` behind, `mod.rs:8105`) would corrupt a later read → parity break.

2. **`UnwrapSome` (`mod.rs:7852`)** — `VmValue::OptionSome(value) => (**value).clone()`.
   Same: clones the inner value out of the borrowed box. A `take_reg(src)` +
   move-out-of-`Box` would be a clean win (one fewer clone, the `Box` free is
   unchanged) **iff `src` is dead** — unknown here.

3. **Option/Result intrinsics (`exec_option_intrinsics` `mod.rs:14103`,
   `exec_result_intrinsics` `mod.rs:14226`)** — e.g. `OptionMap` (14162),
   `OptionAndThen` (14116), `OptionUnwrapOr` (14200) all do `(**value).clone()`
   on an argument obtained via `intrinsic_arg` (`mod.rs:14691`), which returns a
   **read-only `&VmValue` borrow of the shared `self.stack`**. There is no
   scratch/argument region — args alias live registers — and the source kernel
   passes these args with explicit `read` borrows (caller retains ownership). So
   the arg is *known live*; taking it would break parity.

I also checked `OptionFilter` (`mod.rs:14129`): its two clones are both
necessary (one is consumed by the predicate via `call_closure_one`, one is the
kept `Some` body), and it is not on this kernel's hot path. The `SetField`
handler (`mod.rs:7563`) already shows the project's *safe* idiom — `take_reg` the
object, then `write_field_value_owned` mutates in place only when the `Rc` is
uniquely owned — but that works because the object register *is* known dead at
that point (it is rewritten by the same op).

### The follow-up that makes a move safe (the actual Phase-4.3 work)

The blocker is uniformly "is this source register dead after the op?", and the
fix is to **answer that at compile time and carry it in the instruction**:

1. **Add a per-instruction `consume: bool` (or a `last_use` register bitset) to
   the value-moving opcodes** — `MakeSome`, `MakeOk`/`MakeErr` (struct ctor),
   `UnwrapSome`, and the `Option*`/`Result*` `CallIntrinsic` arg list. The reg-VM
   compiler (`RegVm::compile`/`emit` sites, e.g. `mod.rs:4003`, `5240`) already
   sees the full body; a backward last-use scan over each function's `RegInstr`
   stream (the code is already linear with explicit branch targets) yields, per
   register, the IP of its final read. Where a value-moving op is the final read
   of its source, set `consume = true`.
2. **In the interpreter, branch on `consume`:** when set, `take_reg(src)` and
   *move* the inner value out of the `Box`/`Rc` (using `Rc::try_unwrap` /
   `Box`-deref-move) instead of `(**value).clone()`. When clear, keep the
   current clone. This is parity-neutral *by construction*: same value produced,
   one fewer refcount round-trip, and the wrapper drop is unchanged.
3. **Gate it behind the parity suite** (`runtime jit_acceptance` ×2 and the
   `differential` N-way sweep) before enabling, and keep the `consume=false`
   clone path as the conservative default so a liveness bug degrades to *slower*,
   never *wrong*.

Estimated payoff (from 4.1): the option/result cohort spends ~40% of Ir in the
allocator and ~7% more in `drop_in_place`/`Rc::drop_slow`/`clone`; eliminating
the redundant clone+drop on consumed `Some`/`Ok`/`Err` unwraps targets the
largest single slice of that. It needs the liveness pass first, which is why it
is correctly a *next* step rather than a same-session hack.

### Why deferral is the right call here

The task is explicit: "If after Step 2 you judge NO change is safely
implementable without parity risk in the time available, write up 4.3 as
evaluated/deferred … Do NOT force an unsafe or unmeasured change." The reg-VM has
no liveness today; the safe version of this optimization *is* the liveness pass
above, which is a real change deserving its own parity-gated step. No code was
changed, so the parity gate (Step 4) is not required for this step.
