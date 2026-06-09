# JIT Semantics Ledger

Every native-JIT blocker or helper is a question about the language/runtime
boundary: where does a behavior's *definition* live so the interpreter, the
native JIT, and the AOT (Rust) compiler all share one model? This ledger records
that decision per feature, so JIT work hardens the language instead of scattering
optimization patches.

For each feature: current VM behavior, AOT behavior, what the JIT needs, the
semantic gap, the chosen home, the decision, and the tests that pin it.

The homes, narrowest first: **language rule → typed IR metadata → stdlib
contract → VM runtime helper → JIT-only optimization**. The goal is not to move
everything into the language — only to make semantics explicit enough to share.

---

## Cross-cutting models

### Effect tiers (the JIT eligibility model)

The native tier's reach is defined by an *effect classification*, today computed
implicitly by `compute_jit_eligibility` (non-suspending + non-recursive) and
`native_subset_instruction` (the pure scalar/read-heap core). Stated as tiers:

| Tier | Meaning | JIT treatment today |
|------|---------|---------------------|
| `PureScalar` | int/bool/float + control flow, no heap | native machine code |
| `PureReadHeap` | + reads structs/lists (no mutation) | native + checked host helpers |
| `LocalMut` | mutates only locally-owned heap | interpreter (needs alias rules) |
| `RuntimeEffect` | log/file/native/process/net/time/random | interpreter (effect boundary) |
| `Suspending` | async/channel/await/select/sleep | interpreter (no native frame/deopt) |
| `Resourceful` | owns cleanup/drop obligations | interpreter (no cleanup metadata) |

This taxonomy is currently a *documented model*, not a single enum: the first two
tiers are what the native tier compiles; the rest are the fallback boundary. It
will be promoted to an explicit classification when `LocalMut`/`Resourceful`
native support is actually attempted (demand-driven, per the no-speculation rule).

### Helper ABI (the runtime boundary for heap reads)

Native code can't hold heap values in its scalar registers, so it reads them by
calling host helpers (`vm-jit::HostHelpers`, implemented in `reg_vm`). The current
contract:

- Signature: `helper(handle, …) -> i64`. `handle` indexes a per-call table the VM
  fills with the call's heap arguments (`JIT_HEAP_ARGS`).
- Failure: the helper sets a thread-local **bail byte**; generated code loads it
  and branches to fallback *immediately after every helper call*, so a failed
  read can never keep executing. The VM then re-runs on the interpreter.

**Decision (status vs flag):** a binary bail flag, not a `HelperStatus {
Ok|TypeMismatch|Bounds|RuntimeError|Unsupported }` enum. Rationale: the
interpreter re-run *is* the exact error (single source of truth), so distinguishing
failure kinds buys nothing today. Promote to a status enum only if/when native
code reconstructs errors itself (it doesn't). Recorded so the binary flag is a
deliberate choice, not an oversight.

---

## Feature ledger

### Numeric arithmetic (`+ - * / % << >>`, comparisons)

- **VM:** checked — overflow / divide-or-modulo-by-zero / `i64::MIN / -1` /
  out-of-range shift are language-level runtime errors.
- **AOT:** same checked semantics in generated Rust.
- **JIT need:** run unchecked-then-guard; bail to interpreter on any edge.
- **Gap:** none — the native subset is side-effect-free, so re-running reproduces
  the exact error.
- **Home:** language rule (checked arithmetic) + JIT-only optimization (guarded
  native ops).
- **Decision:** native emits `sadd_overflow`/checked div/etc. and bails on the
  edge; never reproduces the error in native.
- **Tests:** `backends_agree_on_*` integer/float generators; force-deopt backend.

### Struct field read — `GetFieldSlot`

- **VM:** field access by slot (declaration-order index resolved at lowering);
  the value is a `VmValue`.
- **AOT:** native Rust struct field access.
- **JIT need:** read an `Int` field of a struct **parameter** without a name hash.
- **Gap:** the field layout is checker knowledge; don't rediscover it at runtime.
- **Home:** typed IR metadata (slot in the bytecode) + stdlib/runtime helper
  (`rss_jit_field_int`).
- **Decision:** the lowerer emits `GetFieldSlot { slot }` (declaration-order,
  with struct construction canonicalized to match); native calls `field_int` on
  the struct handle. Only `Int` fields, only struct **parameters** (handles never
  originate in native code).
- **Tests:** `backends_agree_on_native_heap_reads` (5-way).

### `List.len`

- **VM/AOT:** length of the list; total, no failure.
- **JIT need:** read the length of a list parameter.
- **Home:** stdlib intrinsic contract + runtime helper (`rss_jit_list_len`).
- **Decision:** native calls `list_len` on the list handle. Effects: none; total.
- **Tests:** `backends_agree_on_native_heap_reads`.

### `List.get<Int>`

- **VM:** bounds-checked; returns the element, **runtime error** on out-of-bounds.
- **AOT:** generated Rust indexes the `Vec` — **panics** on out-of-bounds.
- **JIT need:** read an `Int` element of a list parameter at a computed index.
- **Gap/risk:** the failure path. VM errors gracefully; AOT panics; native must
  *not* keep running on a bad read (a read feeding a loop condition could loop
  forever).
- **Home:** stdlib intrinsic contract + runtime helper (`rss_jit_list_get_int`),
  with the immediate-bail ABI above.
- **Decision:** native calls `list_get_int`; out-of-bounds/non-int sets the bail
  byte and native branches to fallback at once → interpreter re-runs → exact
  error. "Failure means fail on every backend" (messages differ; the AOT panic
  and the VM error are both failures).
- **Tests:** `backends_agree_on_native_heap_reads` (success); **failure-path**
  `backends_all_fail_on_out_of_bounds_list_get` and
  `…_list_get_in_condition` (the loop-condition variant guards immediate bail).

---

## Conventions

- **Promote a behavior into a contract only when the JIT needs it** — keep IR
  metadata and helpers demand-driven (the review's "don't move everything").
- **Every promoted contract gets a differential test**, including a
  **failure-path** test (`assert_backends_all_fail`) — those catch hardening bugs
  the success-path differential can't.
- **The interpreter is the single source of truth**: native may only be faster,
  never different, and always falls back rather than reproducing error paths.
