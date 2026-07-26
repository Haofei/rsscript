# RSScript Execution Engine Specification v0.1

The normative contract for RSScript's **register virtual machine (reg-VM)** and
its **JIT tiers** (tier-0 specializing executor + the native Cranelift baseline).

Sections 0–11 are the **durable normative contract** — what MUST hold at every
tier. **Part II** (appendices A–C, non-normative) consolidates the implementation
baseline, the JIT phase status, and the per-feature parity ledger that previously
lived in separate `vm-architecture.md`, `jit-roadmap.md`, and
`jit-semantics-ledger.md` docs; those are now folded in here so the engine has a
single home.

---

## 0. Status and Normative Hierarchy

This document is **subordinate** to the RSScript Language Specification and its
Constitution ([`RSScript_v0.7_Spec.md`](RSScript_v0.7_Spec.md)). Where the
language spec defines an *observable* behavior (effects, freshness, overflow
trapping, error semantics), that definition governs; this spec describes only how
the execution engine *realizes* those behaviors and the engine-internal
invariants that keep the realization faithful.

### 0.0 Authority hierarchy (one ordering, used consistently)

The terms "source of truth" are easy to overload, so this spec fixes a single
ordering and uses it everywhere:

1. **The RSScript language specification defines observable semantics.** It is the
   final authority on what any program is allowed to observe.
2. **The AOT backend (RSScript → Rust → `rustc`) is the shipped execution target
   and the primary conformance target.** When a program runs in production, this
   is what runs.
3. **The HIR interpreter and the reg-VM are executable *parity oracles*** for
   development and differential testing. They exist to make divergence catchable,
   not to define meaning.
4. **No engine tier (interpreter, reg-VM, tier-0, native) may define observable
   semantics independently.** A tier that disagrees with the language spec is
   wrong, whichever tier it is.

"Single source of truth" elsewhere in this document is shorthand for rule 4: there
is exactly **one** semantic definition (the language spec, realized by AOT), and
every tier reproduces it rather than inventing its own. The interpreter's role as
the differential's *reference implementation* (the oracle other tiers are checked
against) is a testing convenience, not a competing authority — where the
interpreter and the language spec/AOT could differ, the language spec wins and the
interpreter is the bug.

The **supreme law of this spec** is the Parity Invariant (§2). Every other rule
here exists to keep it true. A rule in this document that conflicts with the
Parity Invariant is in error.

Normative keywords (MUST / MUST NOT / SHOULD / MAY) carry their usual force.

### 0.1 Relation to the language §21 non-goals

The language spec §21 lists "custom VM as primary execution target", "custom
native backend", "LLVM backend", and "JIT" as **non-goals**. Those are non-goals
of the *language surface and primary execution model*: the engine described here
is **never** the semantic source of truth, is **never** the shipped execution
target for a program's observable behavior (that is the Rust-lowering AOT path),
and exposes **no** surface syntax a program can observe or depend on. The reg-VM
and its JIT are a **development-tier accelerator** whose only contract to a
program is "same observable result as AOT, faster to iterate on." Nothing in this
spec contradicts §21; it specifies the internal engine that §21's non-goals
deliberately keep out of the language surface.

---

## 1. Execution Tiers

RSScript admits a fixed ladder of execution tiers. A program's *observable*
behavior is identical on all of them (§2); they differ only in startup cost and
throughput.

| Tier | What it is | Role |
|------|-----------|------|
| **HIR interpreter** | Tree-walking evaluator over the high-level IR | Reference semantics for early bring-up and parity oracle |
| **reg-VM** | Register virtual machine over a prepared executable | Default development-tier execution |
| **tier-0 JIT** | In-`rsscript` specializing executor reusing the reg-VM's exact value/register operations | Accelerates the numeric/control core with zero new semantics |
| **native JIT** | Cranelift machine-code baseline in the separate `vm-jit` crate | Accelerates eligible scalar, heap-read, and transactionally journaled heap-write regions |
| **AOT** | RSScript → Rust source → `rustc` | The shipped execution target and primary conformance target (§0.0) |

Rules:

1. Per §0.0, the **language spec** defines observable behavior and the **AOT path**
   is the primary conformance target. The interpreter and reg-VM are parity
   oracles, not authorities. Where any tier could differ from the language
   spec/AOT on an observable, the language spec decides and the engine conforms.
2. A faster tier MUST NOT be selected if it cannot preserve §2. Tier selection is
   an optimization decision only.
3. Each tier is **per-function** opt-in: a single run MAY execute some functions
   natively, some in tier-0, and some in the interpreter, switching at call
   boundaries (and, for the native tier, at guard bails — §7).

---

## 2. The Parity Invariant (supreme law)

> For every program and input, within the language-defined observability boundary,
> **HIR-interp ≡ reg-VM ≡ tier-0 ≡ native ≡ native-force-deopt ≡ AOT**
> on all observable behavior: return value, mutation of reachable state, emitted
> output, and success-vs-failure (per the failure-equivalence rule below).

Here **HIR-interp** is the tree-walking HIR interpreter, **reg-VM** is the
register VM running fully interpreted (no JIT), **tier-0**/**native** are the JIT
tiers, **native-force-deopt** is the native tier forced to bail at every guard,
and **AOT** is the compiled backend. "interp" unqualified means the HIR
interpreter. All six are differential backends (§2 consequence 3).

An operation whose language contract deliberately leaves an outcome unspecified
(notably raw `Map`/`Set` traversal order) denotes a set of permitted traces. In
that case parity means that every backend stays within the same permitted trace
set; it does not require byte-identical ordering. Programs that make order
observable and require reproducibility MUST use `SortedMap`/`SortedSet` or sort
the traversed values explicitly. Engine policy supplied outside the program,
including `VmLimits`, is covered separately by §6 and is compared only between
execution modes that accept the same policy.

Consequences (all normative):

1. **One semantic definition, reproduced by all tiers.** Per §0.0 there is a
   single semantic definition (the language spec, realized by AOT). A faster tier
   MUST reuse the reference value representation (`VmValue`) and the shared runtime
   intrinsics, and MUST NOT *independently define* a behavior's semantics. A tier
   MAY emit specialized code for a **closed, explicitly-ledgered** scalar/control
   operation (e.g. guarded native arithmetic, §C.1) when its edge cases are guarded
   and covered by the N-way differential; for everything else it MUST call shared
   runtime helpers or fall back. The test is intent: reproducing the one definition
   faster is allowed; authoring a second definition is not.
2. **Fallback is the correctness floor.** Anything a faster tier does not fully
   support MUST run on a lower tier (ultimately the interpreter). Unsupported
   features therefore cannot create a divergence; only what a tier *does* execute
   can carry a bug — and that is exactly what the differential targets.
3. **The N-way differential is the gate.** `interp`, `tier-0`, `native`,
   `native-force-deopt`, and `compiled` MUST agree on the curated corpus and on
   generatively-produced programs. The backend set and harness are
   `tests/common/differential.rs` and `tests/backend_differential.rs`. No tier
   change may land with the differential red.
4. **Inspectable, non-regressing coverage.** What a tier covers vs falls back MUST
   be inspectable (e.g. `RegVmExecutable::jit_plan`) and version-controlled. The
   intended direction is convergence (the unsupported set shrinks over time), but a
   *deliberate* narrowing — disabling coverage found unsafe, buggy, or not worth
   its risk — is allowed when it is **explicit, justified, and still covered by
   fallback-parity tests**. What is forbidden is a *silent* coverage change in
   either direction.
5. **Determinism of the oracle.** Generated differential programs MUST exclude or
   seed every nondeterminism source (float NaN/ordering, map iteration order,
   time/random/env, scheduling, divide-by-zero) so any observed divergence is a
   real bug, not noise. (This bounds what the *generator* emits; it does not weaken
   the language contract for handwritten programs that use those features; such
   programs remain constrained to the permitted traces described above. See §5
   on float parity by construction.)
6. **Call binding precedes backend lowering.** A call has one canonical binding
   from source arguments to declaration-order parameter slots. The binding records
   source evaluation order, parameter order, defaults, receiver position, and
   same-name shorthand. Checker, HIR/reg-VM, JIT input, and AOT lowering MUST
   consume that binding contract; a backend MUST NOT infer omitted defaults from
   argument count or independently reinterpret argument labels. Constructors and
   multi-field sum variants obey the same rule.

### 2.1 Failure equivalence (what "same failure" means)

Success-vs-failure is observable; raw message text and host panic formatting are
not. Precisely, a failure is **equivalent across tiers** iff:

1. **No further observable effect precedes it.** Every tier fails before producing
   any additional observable effect (output, reachable-state mutation) beyond what
   a successful prefix would have produced identically on all tiers. A tier MUST
   NOT emit a side effect that another tier elides before failing.
2. **Same language-level fault class.** The failure belongs to the same
   language-defined fault class (e.g. integer-overflow trap, divide-by-zero,
   index-out-of-bounds, budget-exhausted). The fault *class* is the language spec's
   to define; this engine conforms to it.

What is explicitly **not** part of parity: backend-specific message strings, host
panic formatting, and which concrete mechanism realizes the fault (an AOT `Vec`
index *panic* and a reg-VM bounds *`EvalError`* are the same failure — same class,
same point, no prior divergent effect).

This is the deliberate contract for v0.1, in which RSScript faults are
non-recoverable at the language surface (a fault terminates the run; it is not a
catchable value with an inspectable kind/span/stack). **If the language ever
exposes error kind, source span, recoverability, or fault-triggered cleanup as
observable**, this rule MUST be tightened to make those properties part of failure
equivalence; "success-vs-failure only" would then be too weak.

---

## 3. Reg-VM Execution Model

The reg-VM executes a prepared executable. Conceptually:

```text
RegVmExecutable
  functions:    Vec<Rc<RegFunction>>     // statically prepared, never cloned per call
  function_ids: map<name, FunctionId>
```

The running VM owns shared, append-only state:

```text
RegVm
  registers: Vec<VmValue>   // one shared register stack for all frames
  frames:    Vec<RegFrame>  // explicit call-frame stack (non-recursive in Rust)
  args, stdout, …           // host I/O surface
```

A frame names a function's window on the shared register stack:

```text
RegFrame { function: Rc<RegFunction>, ip: usize, base: usize }
```

`base` is the index of register slot 0 for this frame. The window is
`base .. base + function.regs`.

Normative model rules:

1. Execution MUST be **frame-based and non-recursive** for ordinary user-function
   calls: a call pushes a `RegFrame`; it MUST NOT recursively re-enter the Rust
   run loop per call. (Runtime intrinsics that synchronously invoke a user
   callback — `map`/`filter`/`fold` — MAY re-enter the same frame loop to receive
   the callback result; this is an implementation seam, not a semantic one.)
2. Functions MUST NOT be cloned per call (a semantic/safety invariant: per-call
   cloning would fork mutable state). Arguments are written directly into the
   callee window (§4). Ordinary calls **SHOULD** avoid per-call argument-`Vec`
   allocation on the hot path — this is a performance target (see §A.2), not a
   correctness rule.

---

## 4. Calling Convention

The reg-VM uses **fixed register windows on a shared, append-only register
stack**. (Historical note: an earlier design truncated a push/pop stack on
return; the engine does not. Windows are reused in place.)

1. Each function has a statically known register count (`regs`); registers
   `base .. base + regs` are its window. **Captures** (for closures) precede
   **parameters**, which precede other locals.
2. The caller chooses the callee's `base` **past its own live registers**, copies
   argument values into the callee's parameter slots, and pushes the frame
   (`function`, `ip: 0`, `base`).
3. Frame preparation grows the shared register stack to `base + regs` **if
   needed**. It MUST only ever *grow*; windows are reused in place, never
   truncated. Preparation MUST clear the per-register *written* bits for the new
   window.
4. `Return { src }` copies the callee's `src` register into the caller's
   destination register and pops the frame. The register stack MUST NOT be
   truncated; the freed window is reused by the next call.

### 4.1 The written-bit invariant (normative)

Each register carries a **`written` bit**, asserted on every read/take. A read of
an unwritten slot — e.g. a stale value left in a reused window by a lowering bug —
MUST fail loudly rather than silently observe garbage. This bit is the engine's
defense against window-reuse aliasing; it MUST NOT be removed as an
"optimization." It is the realization, at the engine level, of the language's
"no observing uninitialized state" guarantee.

**Read-safety is not enough; lifetime-safety is also required.** Because the
register stack is append-only and never truncated on return (§4 rule 4), a reused slot
may *physically* still hold the previous frame's `VmValue` even after its written
bit is cleared. The written bit makes that value unreadable, but on its own it does
not stop the stale `VmValue` from keeping a heap object alive. Therefore, clearing
a register's written bit (on window preparation, and on every path that frees a
slot) **MUST also make the previous slot value non-retaining**: the slot MUST NOT
keep its prior `VmValue` reachable for ownership, drop/cleanup, or memory/resource
accounting (§6.1). Equivalently, the implementation MUST release or overwrite the
slot's prior value when it clears the bit — an unwritten slot is conceptually
empty, not "holding a hidden live value." Without this, `mem_budget` accounting,
leak tests, and any future `Resourceful` value would observe ghost retention.

---

## 5. Value Model

1. The reg-VM and tier-0 operate on `VmValue` — the **single** value
   representation shared by the interpreter, so the two cannot diverge on
   representation.
2. Native (Cranelift) code cannot hold `VmValue`s; it unboxes registers into
   scalar slots **by storage class**, fixed per register by the JIT IR's
   `reg_types`:
   - `Int`, `Bool` → `i64` (Bool as `0`/`1`)
   - `Float` → `f64`, passed as its `to_bits` pattern
   Results box back by the same class. The same arithmetic/compare opcode lowers
   to integer **or** float machine ops purely by operand class.
3. **Float parity is by construction, within a pinned observability boundary.**
   Native float arithmetic emits the plain IEEE-754 ops (`fadd`/`fsub`/`fmul`/`fdiv`)
   and **ordered** comparisons (NaN → false), which are exactly Rust's `f64`
   `+ - * /` and `< <= > >=` and the interpreter's `VmValue::Float` operations.
   Float division never traps (`x/0.0` = `±inf`/`NaN`), so floats need no guard.
   This makes the tiers bit-identical **only for the observables RSScript pins**;
   to keep the claim honest, the boundary is stated explicitly:
   - **`Float` register payload is the full `f64` `to_bits` pattern**, carried
     verbatim across tiers — so a NaN's payload and sign bits are *preserved* by
     arithmetic/copy exactly as Rust preserves them. No tier canonicalizes NaNs.
   - **`==`/`!=` on `Float` follow Rust `f64` exactly**: `NaN == NaN` is `false`,
     `NaN != NaN` is `true`, `+0.0 == -0.0` is `true`. (`==` is *not* the
     bit-comparison; bit-level identity is observable only via an explicit
     `to_bits`-style operation, if and when the language exposes one.)
   - **Printed float formatting is part of parity**: all tiers MUST format a given
     `f64` identically (same shortest-round-trip rendering, same `inf`/`NaN`
     spelling). Formatting divergence is a parity bug, not an allowed difference.
   - **Reconciliation with §2 consequence 5:** the *generator* excludes/seeds NaN and ordering
     so generated programs stay deterministic oracles; that is a property of the
     test generator, not a weakening of this guarantee. **Handwritten** programs
     that produce NaN/±inf are fully in-contract and MUST agree across tiers under
     the boundary above. The curated corpus MUST include seeded-NaN/±inf cases to
     pin this directly rather than relying on the generator.

---

## 6. Sandbox and Hardening Contract

The reg-VM is a sandbox for untrusted/AI-generated programs. Two invariants hold.

**Invariant 1 — embedded VM/JIT program faults are values, never panics.** In the
interpreter, tier-0, native JIT helpers, and embedding APIs, a program-level fault
(overflow trap, divide-by-zero, out-of-bounds, step/memory-budget exhaustion,
cancellation) MUST be returned as an `EvalError` value, never a Rust panic.
Standalone generated-Rust AOT executables may use a controlled Rust panic/abort
as the backend mechanism described by §2.1, because v0.1 exposes no catchable
language-level fault value there. Such a panic is equivalent only when it has the
same fault class and semantic point and no additional observable effect precedes
it.

**Invariant 2 — a Rust panic inside an embedded VM/JIT boundary means an engine
bug, not a program fault.** The release profile sets `panic = "abort"` (workspace
`Cargo.toml`, `[profile.release]`), so a panic can never unwind across the C ABI
at the JIT / FFI / native-helper seams (that would be undefined behavior). The `rsscript` crate is
`#![forbid(unsafe_code)]` (`lib.rs` and `main.rs`); all `unsafe` (machine-code
execution, indirect calls, symbol registration) is confined to the `vm-jit`
crate behind a safe API (§7.1).

### 6.1 Resource limits (`VmLimits`)

The register-VM execution API accepts a `VmLimits` budget enforced by the
interpreter, tier-0, and native JIT. `VmLimits` is host execution policy, not a
source-language value and not part of standalone generated-Rust AOT semantics;
the AOT command has no `VmLimits` input. AOT remains subject to the host process's
own resource controls. Runs may be compared for resource-limit parity only when
the compared execution modes accept the same configured policy.

| Field | Meaning | Default |
|-------|---------|---------|
| `max_depth` | Max logical call depth. Checked before every frame push and every optimized self-tail-call. | Generous, default-on |
| `step_budget` | Max executed instructions over the whole run; stops `while true {}`. | `None` (unlimited) |
| `mem_budget` | Best-effort cumulative quota for VM-owned allocation and container-capacity growth. It is not a live-RSS measurement. | `None` (no accounting) |
| `cancel` | Host preemption flag (e.g. watchdog/timeout); checked at a throttled step tick, even inside a tight non-awaiting loop. Preempts the whole eval with a "cancelled" runtime error. | `None` (near-free off path) |
| `stdout_budget` | Max total bytes a program may write to captured stdout (all `Log.write`/trace paths funnel through one append point); the write that would exceed it fails cleanly *before* appending. Stops an output flood that `step_budget` (silent loops) does not. | `None` (unlimited) |
| `host_call_budget` | Max stdlib/runtime intrinsic dispatches — the `Type.method` boundary out of pure VM bytecode into host library code, where all file/process/network/clock/logging effects enter. Caps the *volume* of host calls independently of instruction count (one intrinsic can do unbounded I/O). | `None` (uncounted) |

The off path for each limit MUST stay near-zero-overhead (no atomic touch when
`cancel` is `None`, no accounting when `mem_budget` is `None`, a single
unconditional counter increment for `step_budget`/`host_call_budget`), so
hardening is free for trusted runs. `host_call_budget` is charged once at each of
the two intrinsic-dispatch entry points, so it counts identically whether a call
is reached via the interpreter or the tier-0 executor (a JIT-eligible or
native-eligible function never contains an effectful intrinsic, so there is no
tier on which the count can diverge).

Tail-call optimization may reuse a physical frame, but it does not erase logical
calls from `max_depth`. The interpreter carries a per-frame tail-call counter; the
native tier receives the current logical depth and guards each optimized tail edge.
An accepted OSR exit writes the resulting logical depth back into that frame's
tail-call counter; a guard bailout that will be replayed discards the speculative
native count.
An exceeded native guard falls back anonymously so interpreter replay emits the
canonical depth-limit error rather than resuming with a partially counted frame.

Structural Map/Set key validation and hashing consume step budget proportional to
the traversed nodes/bytes, with bounded key depth and size. Hash tables use a
per-process randomized hasher; deterministic iteration is not a language contract.
Container capacity growth and size-parameterized intrinsic outputs are charged to
`mem_budget` before they become reachable when armed. The counter is cumulative:
dropping or replacing a value does not refund prior allocation. Fresh VM-owned
List/Map/Set/String/Bytes/JSON/object results are charged, including values returned
by intrinsics and native bindings. Native bindings may allocate host memory while
running; `mem_budget` governs only values accepted into VM ownership, so hostile
execution still requires an OS process/container memory limit.

### 6.2 Limits bind every VM tier, including native code (normative)

Within one register-VM run, `VmLimits` constrain the *program's* execution, so
they MUST hold no matter which VM tier runs a given function. A faster tier cannot
escape a budget by bypassing the VM instruction loop. Concretely, native (and
tier-0) code MUST satisfy **both**:

1. **Enforce, or be ineligible.** While any of `step_budget`, `mem_budget`, or
   `cancel` is active, a function compiles to native **only if** the native code
   emits the equivalent check on **every loop backedge** (and at a bounded
   instruction/basic-block interval for straight-line growth) — the same points
   the interpreter would tick. A native-compiled `while true {}` MUST observe
   `step_budget`/`cancel` and terminate just as the interpreter would. If a tier
   cannot emit an equivalent check for an active limit, that limit makes the
   function **ineligible** and it MUST fall back. A limit MUST NOT be silently
   dropped by running natively.
2. **No double-counting across a bail.** Any budget bookkeeping a native attempt
   performs before it bails (§7.2) MUST be either non-observable or **restored to
   the pre-attempt value before the interpreter re-runs**. The engine MUST NOT
   charge `step_budget`/`mem_budget` twice for the one logical execution of a
   bailed function. The simplest conforming implementation snapshots the budget on
   native entry and restores it on bail; charging the budget only on the
   interpreter path (native runs "off the meter" until it commits a result) is
   equally conforming. Either way, the observable budget after a bailed-then-
   re-run function MUST equal what a pure-interpreter run would show.

This is the same equivalence argument as §7.2 extended to resource accounting: a
bail must leave **no** observable trace, and budget consumption is observable
(it can change success-vs-failure).

**Current implementation (Model A — ineligibility).** The engine takes rule 1's
fallback branch directly: `try_native`/`try_osr` refuse to dispatch to native code
whenever `step_budget`, `cancel`, **or `mem_budget`** is armed, so a preemptible or
memory-limited function always runs on the tier-0/interpreter path that `tick()`s
every instruction. `mem_budget` is in this gate because the native subset can now
mutate/allocate heap (§7.1 rule 9 write helpers): an allocating native attempt runs
off the meter and would otherwise grow the accounted live-set past the limit without
erroring. Rule 2 holds by construction: a native attempt never calls `tick()` and
never touches the step/mem counters, so it runs entirely "off the meter" — a bail
leaves the budget at its pre-attempt value and the interpreter re-run charges it
exactly once. Emitting per-backedge checks in Cranelift (rule 1's
first branch, which would let hot loops stay native under an active budget) is a
future optimization; until then preemptible runs simply forgo the native tier.

---

## 7. JIT Eligibility and the Pure Subset

The native tier's reach is an **effect classification**. Today it is computed by
eligibility, instruction descriptors, alias checks, and transaction requirements;
stated as tiers:

| Tier | Meaning | Engine treatment |
|------|---------|------------------|
| `PureScalar` | int/bool/float + control flow, no heap | native machine code |
| `PureReadHeap` | + reads of struct fields / list elements | native + checked host helpers (§7.1) |
| `TransactionalHeapMut` | declared existing-heap or mutable-flat writes with complete alias proof and undo/snapshot coverage | native transaction; commit on success, restore on bail (§7.1–§7.2) |
| `UnjournaledMut` | mutation without a complete rollback strategy | interpreter |
| `RuntimeEffect` | log/file/native/process/net/time/random | interpreter (effect boundary) |
| `Suspending` | async/channel/await/select/sleep | interpreter (no native frame/deopt) |
| `Resourceful` | owns cleanup/drop obligations | interpreter (no cleanup metadata) |

The first three tiers are what native may compile; the rest are the fallback boundary.
This taxonomy is a **documented model**, not yet a single enum; it MUST be
promoted to an explicit classification only when `LocalMut`/`Resourceful` native
support is actually attempted (demand-driven — no speculative machinery).

**Eligibility gate.** A function compiles natively only when **every parameter's
runtime value matches its declared register class** (an `Int` register actually
holds an `Int`, etc.); otherwise it falls back. The `vm-jit` crate independently
re-validates the IR (`vm-jit::validate`) before codegen, so a producer bug fails
as a clean `JitError`, never a panic or miscompile.

**`JitError` is a fallback signal, never a program fault.** A validation or
codegen failure means *the optimizer could not safely compile this function* — it
says nothing about whether the program is correct. In production execution, a
`JitError` MUST therefore **disable that native attempt and fall back** to a lower
tier; it MUST NOT surface as a program failure or change the run's
success-vs-failure (that would let an optimizer bug fail a valid program — a parity
violation). `JitError` MAY be surfaced loudly in diagnostic/test/force-JIT modes,
where a translation bug should be a hard failure rather than a silent fallback.

**Tiering.** A per-function hot-call counter (`tier_up_threshold`) defers native
compilation until a function is hot.

**OSR (on-stack replacement) — implemented experimentally.** The method-at-a-time model
leaves one class unserved: a function called once (or rarely) whose *inner loop*
is hot never crosses a per-call threshold, so the hot loop stays interpreted (a
measured cliff in the VM/JIT benchmark suite). OSR-entry addresses it: when a
loop backedge is observed hot mid-execution, control transfers from the
interpreter into native code **at the loop header**, not only at function entry.

OSR-entry is the **dual of deoptimization** (§7.2). Deopt maps a compiled frame's
state *back* to the interpreter at a safepoint; OSR maps the interpreter's register
window *forward* into a compiled entry at a loop header. Both use the same
per-safepoint live-state description (which registers are live, their storage
classes, the resume `ip`). An OSR-entry compilation begins at the loop-header
block, loads each live register from the interpreter window into its native
location, and runs to completion (or deopts back per §7.2 on any guard).

**Parity (normative).** OSR MUST NOT weaken §2: the transferred state is identical
to the interpreter's at the loop header, the compiled code computes the same values
(it is the same lowering, entered at a different block), and any guard failure
deopts into the interpreter (the reference semantics) — so the result is identical
to running the loop interpreted. Float formatting stays bit-identical, and OSR
observes the SAME `Map`/`Set` object as the interpreter, so their iteration order
is unchanged by tiering. (This is a within-backend guarantee only: `Map`/`Set`
iteration order is *not* pinned across backends — it is implementation-defined
nondeterminism, see the seeding note in §2. Programs needing a deterministic order
across backends MUST use `SortedMap`/`SortedSet` or sort explicitly; the
differential harness does not compare raw `Map`/`Set` iteration order.)
The hot-loop backedge counter guides only *whether/when* to OSR, never the values
computed (determinism, per §2). Automatic OSR is enabled for eligible hot loops;
forced entry points exercise it deterministically in tests. Unsupported state or
armed limits make a region ineligible and keep execution in the interpreter.

### 7.1 The host-helper ABI (the heap-read boundary)

Native code cannot hold heap values in scalar registers, so it reads them by
calling **host helpers** (`vm-jit::HostHelpers`, implemented in `reg_vm`). The
contract is normative:

1. **Read helpers are side-effect-free.** A *read* helper looks a handle up in a
   per-call table and returns one `Int` (a struct `Int` field, a list length, or
   an `Int` list element). A read helper MUST NOT mutate heap, allocate observable
   state, call back into user code, or perform I/O. *Write* helpers (rule 9) are
   the sole exception and are confined to the transactional boundary defined there;
   together these keep the compiled subset **transactionally** side-effect-free —
   the premise of the fallback proof (§7.2).
2. **Handles are opaque, native-attempt-scoped references.** Input handles index
   the attempt's heap-argument table; speculative output handles index the
   transaction's output table. Both tables are owned by the VM and cleared by a
   drop guard on every exit path. Nested native calls share this attempt context.
   A handle MUST NOT outlive the attempt or alias another attempt's tables.
   Native code MUST NOT dereference a raw heap pointer; it passes only `i64`
   handles the VM owns.
3. **Unsatisfiable read = bail, not UB.** Out-of-range, wrong-shape, or non-`Int`
   reads MUST be bounds-checked and type-matched and take the **bail** path
   (below), deterministically — never undefined behavior.
4. **Immediate bail.** A helper signals failure via `vm_jit::signal_bail()`,
   which sets a per-thread bail byte owned by `vm-jit`. Generated code MUST load
   that byte and branch to fallback **immediately after every helper call**, so a
   failed read can never keep executing (a bad read feeding a loop condition could
   otherwise loop forever).
5. **No raw pointers cross the public API.** Host helpers cross as **typed**
   `extern "C"` function pointers; the bail flag is owned by `vm-jit` (its `call`
   resets it and passes its own address inward). A safe caller can supply neither
   a bad helper address nor a dangling bail pointer. The only `unsafe`
   (symbol registration, the indirect call) is private to `vm-jit`.

6. **The signed handle encoding is checked.** `h >= 0` denotes input-table index
   `h`; `h < 0` denotes speculative output-table index `-(h + 1)`. Both lookups
   MUST be bounds checked. An out-of-range value or a stale handle from a prior
   attempt MUST bail. Native code MUST NOT perform arithmetic on handles; only VM
   helpers may mint output handles.
7. **Return value is defined only when the bail flag is clear.** A helper's `i64`
   result is meaningful **iff** the bail byte is clear after the call. On bail the
   return value is unspecified and MUST NOT be observed (rule 4 already requires the
   branch-to-fallback to come first). List length is returned as a non-negative
   `i64`; the conversion from the host length type MUST be exact, and a length that
   does not fit `i64` (not reachable under the VM's container limits) MUST bail
   rather than wrap or truncate.
8. **Nested compiled calls share one transaction context.** Helpers do not call
   back into user code (rule 1). A generated `CallNative` may invoke another
   compiled function using the same input/output tables, bail byte, and heap
   transaction. Child bailout propagates to the outer attempt. A compiled call
   with a mutable heap-handle parameter is currently ineligible because the call
   ABI has no mutable-result slot with which to rewrite the caller's live handle;
   such calls execute in the interpreter until that ABI is defined. Mutable flat
   buffers use their separate pointer/length ABI and remain eligible.

9. **Write helpers are transactional (journaled, commit-on-success).** A *write*
   helper may mutate heap state (a struct field, a list element/length, a
   map/set/deque) ONLY through the heap-write transaction. Before the first
   mutation of a container it MUST record an undo entry — a snapshot of the
   pre-attempt container, keyed by its root — into the per-call transaction
   journal, and stage the resulting handle as a deferred write-back. The mutation
   is NOT observable to the rest of the VM until the transaction **commits**, which
   happens only when the native attempt completes cleanly. On **bail** the
   transaction **aborts**: every journaled undo is restored, so the heap returns to
   its exact pre-attempt state. A native attempt that performed any heap write
   therefore disables precise mid-function deopt resume (it re-runs the whole
   function on the interpreter from the top), making commit-or-abort whole-attempt
   atomic. This is the §7.2 checkpoint/rollback equivalence argument made concrete.

The bail signal is a **binary flag**, not a `HelperStatus` enum. Rationale: the
interpreter re-run *is* the exact error (single source of truth), so
distinguishing failure kinds buys nothing today. Promote to a status enum only
if/when native code reconstructs errors itself.

### 7.2 Fallback equivalence (the proof obligation)

Because native heap and mutable-flat effects are confined to the
commit-on-success transaction of §7.1 rule 9,
a bail at *any* point — an arithmetic guard, a divide-by-zero edge, a helper that
cannot satisfy a read, or one that has already journaled a heap mutation — leaves
**no observable effect** behind: the transaction aborts and restores any touched
heap to its pre-attempt state. Re-running the whole function on the interpreter is
therefore indistinguishable from never having attempted native execution: same
inputs, same (restored) heap, same result or same error.

This is why a binary bail flag suffices and why the native tier **can only ever
be faster, never semantically different**. The original read-only subset met this
trivially; the heap-write extension meets it via the §7.1 rule 9
checkpoint/rollback transaction (journaled undo, commit-on-clean-completion,
abort-on-bail) plus its differential + force-deopt coverage. Any *further*
extension that lets native code perform an effect the transaction cannot undo —
I/O, observable allocation visible before commit, or a call back into user code —
BREAKS this proof and MUST NOT land without its own replacement equivalence
argument and differential coverage.

**"No observable effect" includes VM bookkeeping.** Any state a failed native
attempt touched before bailing — most importantly `step_budget`/`mem_budget`
accounting (§6.2) — MUST be either non-observable or restored to its pre-attempt
value before the interpreter re-runs. A bail that leaks budget consumption is a
parity bug, because budget consumption can change success-vs-failure.

**Deopt is tested by always deopting.** A `force-deopt` backend (native bails at
every guard) is a permanent member of the N-way differential, so
`{interp, tier-0, native, force-deopt, compiled}` must agree on every program in
the configured differential corpus. Force-deopt exercises every native guard that
the selected program reaches; coverage is expanded per feature and is not a claim
that every possible source/state combination is already generated.

---

## 8. Per-Feature Parity Ledger

Every behavior promoted into the JIT (a new IR opcode, a new host helper, a new
eligible tier) is a decision about **where that behavior's definition lives** so
all tiers share one model. The homes, narrowest first:

> language rule → typed IR metadata → stdlib contract → VM runtime helper → JIT-only optimization

**The conventions in this section are normative; the ledger of entries is not.**
This section (§8) states the binding rules for *how* a behavior may be promoted
into the JIT. **Appendix C** (Part II) is the running, demand-driven **audit
record** of the individual decisions made under those rules — current VM behavior,
AOT behavior, the JIT need, the semantic gap, the chosen home, the decision, and
the pinning tests. An Appendix C entry **describes** a current implementation
decision; it does not by itself create a new binding contract. A behavior's
contract becomes normative only when it is **promoted into §§0–11 or the language
spec** (with its differential/failure-path tests in place). So §8 governs; Appendix
C records.

Normative conventions (part of this contract):

1. **Promote on demand only.** A behavior becomes IR metadata / a helper / a
   contract **only when a tier actually needs it**. No speculative machinery.
2. **Every promoted contract gets a differential test**, and where the behavior
   has a failure path, a **failure-path** test (`assert_backends_all_fail`) — those
   catch hardening bugs the success-path differential cannot (e.g. an
   out-of-bounds list read feeding a loop condition).
3. **One definition, reproduced (per §0.0).** A faster tier may only be faster,
   never semantically different, and MUST fall back rather than reproduce an error
   path itself. The interpreter serves as the differential's reference
   implementation; it is an oracle, not a competing authority (§0.0 rule 4).

---

## 9. JIT IR and Crate Boundary

1. The native tier consumes a **stable, versioned IR** defined by `vm-jit`
   (`JitInstr`/`JitFunction`, `vm_jit::IR_VERSION`, currently `24`). `rsscript`
   *translates* eligible `RegFunction`s into that IR rather than exposing its
   private `RegInstr`. The two layers MUST stay decoupled across this IR; a
   producer/consumer version mismatch is an error, not a silent miscompile.
2. `vm-jit` MUST re-validate received IR (`validate`) before codegen.
3. Codegen is Cranelift (`cranelift-jit`/`-frontend`/`-codegen`/`-native`) at
   `opt_level=speed`. Source-level constant folding / intrinsic inlining MAY be
   layered on later, each behind the §2 differential gate.

---

## 10. Engine Non-Goals

Within the engine (distinct from the language §21 list):

- **Unrestricted OSR coverage** — OSR is implemented for eligible natural loops,
  but unsupported state, effects, or armed limits deliberately keep execution in
  the interpreter (§7). Multi-entry/irreducible-region OSR remains a non-goal.
- **Un-journaled native side effects before a bail** — forbidden; breaks the §7.2
  fallback proof. Heap *writes* are permitted only through the §7.1 rule 9
  commit-on-success transaction, which restores them on bail; I/O, observable
  allocation, and callbacks into user code remain forbidden before commit.
- **A `HelperStatus` failure-kind enum** — not until native code reconstructs
  errors itself (§7.1).
- **Independently defining instruction semantics in any faster tier** — forbidden by §2 consequence 1.
- **Widening the native subset without extending the generator** to exercise the
  new surface — forbidden by §2 consequence 4.

---

## 11. Change Discipline

1. Do not independently define instruction semantics in a faster tier (breaks §2
   consequence 1).
2. Do not add an optimization without the N-way differential **and** the
   generative fuzz passing (§2 consequence 3).
3. Do not widen the JIT-supported instruction set without extending the program
   generator to exercise it (§2 consequence 4).
4. Do not remove the written-bit (§4.1), the `panic=abort` seam, or the
   `forbid(unsafe_code)` boundary (§6) as "optimizations."
5. Any change touching observable behavior is the **language spec's** call first;
   this engine conforms.

---
---

# Part II — Implementation status & audit (non-normative)

Appendices A–C are **non-normative**: they record where the implementation stands,
how it got here, and the per-feature decisions behind the JIT. The binding rules
are §§0–11 above; where an appendix and a numbered section disagree, the section
governs. A decision recorded in an appendix becomes binding only when it is
promoted into §§0–11 or the language spec (§8). These appendices consolidate the
former `vm-architecture.md`, `jit-roadmap.md`, and `jit-semantics-ledger.md`.

---

## Appendix A. Reg-VM implementation baseline & migration history

The normative execution model, calling convention, written-bit invariant, and
hardening contract are §3–§6. This appendix records the implementation discipline
and the migration that produced them.

### A.1 Feature-gate discipline

The VM must not grow broad new language/runtime feature coverage until the
frame-based execution skeleton is in place and benchmark-covered. The skeleton:
explicit `RegFrame`; a single shared register stack for locals and operands;
non-recursive user-function call/return; closure calls routed through the same
frame machinery; the existing benchmark matrix still passing.

After the skeleton, features are added **in groups**, and a group lands only with:
interpreter-parity tests, VM coverage accounting, a focused benchmark in
`benchmarks/micro/`, and inclusion in `benchmarks/micro/run-matrix.sh` if it is
part of the default VM performance story.

### A.2 Benchmark gates

Release-built benchmark driver commands:

```sh
cargo run --quiet --release --bin rss -- bench --json --mode vm-internal <file> -- <size>
cargo run --quiet --release --bin rss -- bench --json --mode release-internal <file> -- <size>
```

The matrix compares `vm-internal` against `release-internal`. Programs used for
the release comparison must carry data dependencies that prevent the generated
Rust backend from folding the whole workload into a closed-form result.

### A.3 Migration history (done)

The engine moved from a temporary recursive VM to the frame-based register VM in
this order; all steps are complete:

1. Keep the minimal VM feature set stable.
2. Introduce `RegFrame` + shared-stack call/return for ordinary user functions —
   the run loop pushes frames instead of recursively invoking Rust per call.
3. Route closure calls through the same frame machinery — `call_closure` pushes
   captures and arguments onto the shared stack and runs until the previous frame
   depth, with no temporary argument vector for captured closures.
4. Remove recursive `run_function` as the normal path. Ordinary calls are
   non-recursive; closure calls from runtime intrinsics still re-enter the same
   frame loop so `map`/`filter`/`fold` can synchronously receive callback results.
   Making collection iteration itself frame-native remains a later runtime
   state-machine step.
5. Re-run the benchmark matrix and record the ratios.
6. Resume feature migration only after the architecture is stable.

---

## Appendix B. JIT phase status

Built verification-first; the governing invariants are §2 (parity) and §7
(eligibility/ABI). Phases:

- **Phase 0 — N-way differential framework. Done.** `Backend` trait
  (interp/jit/compiled), `assert_backends_agree`, generative differential.
- **Phase 1 — tier-0 JIT (specializing executor). Done.** `RegVm::run_jit`
  executes JIT-eligible functions (the numeric/control core) via a specializing
  loop that reuses the interpreter's exact value/register operations
  (`eval_numeric_binary`/`eval_numeric_compare`/`reg`/`set_reg`/…), so it is
  gap-free by construction. Integrated into the frame loop with per-function
  fallback. The pure subset has a single shared implementation (`try_exec_pure`);
  coverage spans heap/field/match, collection get/set, and cross-function calls.
- **Phase 2 — native (Cranelift) codegen. Done.** Lives in the separate `vm-jit`
  crate behind a safe API (§6, §7.1). ~64× faster than the interpreter on numeric
  kernels. Cranelift at `opt_level=speed`; the stable versioned IR is
  `JitInstr`/`JitFunction` (`IR_VERSION`); value glue unboxes by storage class;
  heap reads and declared transactional writes go through `extern "C"` host helpers;
  direct mutable-flat writes are snapshotted by the embedding VM. Any unsatisfied
  guard bails, rolls back, and replays in the interpreter. Eligibility requires every
  parameter's runtime value to match its declared register class;
  `vm-jit::validate` independently re-checks the IR before codegen.
- **Phase 3 — tiering / deopt / fuzz. Done (baseline).** Per-function hot-call
  counter (`tier_up_threshold`) defers native compilation until hot; experimental
  OSR-entry is implemented for eligible natural loops (§7), while unsupported state
  shapes remain staged. Native bails at every arithmetic guard and the interpreter
  re-runs from the original args; a permanent `force-deopt` backend exercises the
  bail path across the configured differential corpus. This is not a claim that
  every source/state combination is generated. A total `seed(bytes) -> program` decoder
  (`program_from_seed`) drives the differential via proptest seeds and shrinking;
  wiring it to a coverage-guided engine (cargo-fuzz/libFuzzer) is the remaining
  deployment step (the decoder is already total, as such engines require).

Future optimizations (source-level constant folding / intrinsic inlining) may be
layered on later, each behind the §2 differential gate.

### B.1 Readiness ladder (scope-calibrated)

The execution engine is a **development tier**: the reg-VM and its JIT tiers exist
to run RSScript fast and to *cross-check* the shipped AOT target (§0.0), not to be
the deployed runtime. "Production-grade" is therefore calibrated to that scope —
the bar is **soundness and parity**, not feature completeness of the optimizer.
The ladder below states what each level *guarantees*, so a reader can tell what is
actually trustworthy from what is merely fast.

| Level | Guarantee | Status |
|-------|-----------|--------|
| **L0 — Runs** | Tier produces a result for its supported subset; unsupported constructs fall back, never miscompile. | Done (all tiers). |
| **L1 — Differentially equal** | Tier's output is byte-identical to the interpreter oracle across the corpus + generative differential, including the failure path (`assert_backends_all_fail`) and the force-deopt twin. | Done (§2, Appendix C). |
| **L2 — Sandbox-sound** | Every active `VmLimits` (depth/step/mem/cancel/stdout/host-call) binds the tier or makes it ineligible (§6.2); no tier can bypass a budget by running faster. Hostile-input suite + native-limit gate pin this. | Done. |
| **L3 — Robust under malformed input** | Arbitrary/mutated IR into `vm-jit::validate` yields a clean `JitError` (never panic/UB/miscompile); host helpers tolerate bad handles/shapes/indices by bailing. Structured fuzz pins this. | Done (baseline); coverage-guided engine is the remaining deployment step (Phase 3). |
| **L4 — Continuously gated** | Every change re-runs the L1–L3 gates in CI; a new opcode/feature is not "done" until it has a parity entry and a failure-path case. | Process gate (§11, B.2). |

This ladder is deliberately **not** a feature roadmap (more opcodes, more
inlining): those are L0 breadth and are layered in behind the same gates. A tier
that is fast but only at L0/L1 for a construct is still safe to ship *because* the
unsupported remainder falls back — breadth is optional, parity and sandbox
soundness are not.

### B.2 Differential release gate

A change to any tier (interpreter, tier-0, native, or the AOT lowering) MUST clear
this gate before it is trusted:

1. **Parity (L1).** `backend_differential` (corpus + generative, all backends incl.
   native + force-deopt) is green, and every newly supported construct has both an
   agree-case and, where it can fail, a failure-path case (`assert_backends_all_fail`).
2. **Sandbox (L2).** The `hostile` suite is green, and any new execution path that
   can loop or allocate is reachable by the relevant budget (or is documented as
   making the function ineligible for the faster tier).
3. **Robustness (L3).** The `vm-jit` validate/compile and host-helper fuzz tests are
   green; a new `JitInstr` variant extends the fuzz generator.
4. **Ledger (L4).** A new feature has an Appendix C entry and, if it changes a
   normative contract, a §8/§6/§7 edit in the same change (§11).

A red gate is a **fallback or revert**, never a silent ship: a parity failure means
the faster tier is disabled for that construct until the divergence is resolved,
exactly as a runtime `JitError` falls back rather than failing the program (§7).

---

## Appendix C. Per-feature parity ledger

This is an **audit ledger**, not a normative section. Its entries describe current
implementation decisions made under the §8 conventions; the *binding* contracts are
the promoted rules in §§0–11 and the language spec (a ledger entry becomes binding
only when so promoted — §8). The §8 conventions remain in force here: a behavior is
promoted **only when a tier needs it**, and every promotion gets a differential test
plus, where it has a failure path, a failure-path test. For each promoted behavior:
current VM behavior, AOT behavior, what the JIT needs, the semantic gap, the chosen
home (narrowest first: language rule → typed IR metadata → stdlib contract → VM
runtime helper → JIT-only optimization), the decision, and the pinning tests.

### C.1 Numeric arithmetic (`+ - * / % << >>`, comparisons)

- **VM:** checked — overflow / divide-or-modulo-by-zero / `i64::MIN / -1` /
  out-of-range shift are language-level runtime errors.
- **AOT:** same checked semantics in generated Rust.
- **JIT need:** run unchecked-then-guard; bail to interpreter on any edge.
- **Gap:** none — arithmetic has no committed side effect, and any surrounding
  journaled mutation is rolled back before re-running to reproduce the exact error.
- **Home:** language rule (checked arithmetic) + JIT-only optimization (guarded
  native ops).
- **Decision:** native emits `sadd_overflow`/checked div/etc. and bails on the
  edge; it never reproduces the error in native code.
- **Tests:** `backends_agree_on_*` integer/float generators; force-deopt backend.

### C.2 Struct field read — `GetFieldSlot`

- **VM:** field access by slot (declaration-order index resolved at lowering); the
  value is a `VmValue`.
- **AOT:** native Rust struct field access.
- **JIT need:** read an `Int` field of a struct **parameter** without a name hash.
- **Gap:** the field layout is checker knowledge; don't rediscover it at runtime.
- **Home:** typed IR metadata (slot in the bytecode) + runtime helper
  (`rss_jit_field_int`).
- **Decision:** the lowerer emits `GetFieldSlot { slot }` (declaration-order, with
  struct construction canonicalized to match); native calls `field_int` on the
  struct handle. Only `Int` fields, only struct **parameters** (handles never
  originate in native code).
- **Tests:** `backends_agree_on_native_heap_reads` (5-way).

### C.3 `List.len`

- **VM/AOT:** length of the list; total, no failure.
- **JIT need:** read the length of a list parameter.
- **Home:** stdlib intrinsic contract + runtime helper (`rss_jit_list_len`).
- **Decision:** native calls `list_len` on the list handle. Effects: none; total.
- **Tests:** `backends_agree_on_native_heap_reads`.

### C.4 `List.get<Int>`

- **VM:** bounds-checked; returns the element, **runtime error** on out-of-bounds.
- **AOT:** generated Rust indexes the `Vec` — **panics** on out-of-bounds.
- **JIT need:** read an `Int` element of a list parameter at a computed index.
- **Gap/risk:** the failure path. VM errors gracefully; AOT panics; native must
  *not* keep running on a bad read (a read feeding a loop condition could loop
  forever).
- **Home:** stdlib intrinsic contract + runtime helper (`rss_jit_list_get_int`),
  with the immediate-bail ABI (§7.1).
- **Decision:** native calls `list_get_int`; out-of-bounds/non-int sets the bail
  byte and native branches to fallback at once → interpreter re-runs → exact error.
  "Failure means fail on every backend" (the AOT panic and the VM error are both
  failures; messages differ).
- **Tests:** `backends_agree_on_native_heap_reads` (success); failure-path
  `backends_all_fail_on_out_of_bounds_list_get` and `…_list_get_in_condition`
  (the loop-condition variant guards immediate bail).
