# Native JIT contract

The native JIT is an optional accelerator for trusted, in-process execution. It
does not add an isolation boundary and never changes Provider authority.

The supported `native-jit` feature is intentionally a bounded feature surface.
Whole-function, transformed OSR, and continuation entries consume the same
explicit source-origin/cost records and all account source steps, armed or not.
When step, cancellation, or deadline controls are armed, generated code reserves
exact source cost by bounded control segment and polls cancellation plus the VM's
typed monotonic-deadline helper at most every 512 source steps (and at loop
backedges). A preemption poll runs before reserving the next segment, so deopt
resumes at the first unpaid source instruction. With no control armed there is
nothing to reserve against or poll, so a region charges each basic block once at
its leader and each bail site corrects the count by a compile-time constant.
Allocation and live-memory controls are admitted only with a per-region proof.
Scalar/read-only whole functions and continuations cannot grow storage. OSR may
also execute shape-preserving stores and `List.push`: the helper charges the exact
capacity delta into a transaction-local allocation cell, live memory is measured
from the tentative VM root graph, and both are committed only with the heap
transaction. Every other allocating/replacing helper and native-call edge fails
closed. A spliced-in callee body owns its own source steps, and a deopt inside
one rolls that region's charge back to the caller's call instruction, which the
interpreter re-executes. A native-to-native call edge compiles its callee with the caller's controls and
charges the same limits cell. A region whose source cost still cannot be
attributed exactly — an OSR loop containing a dissolved call, or a key whose hash
work is proportional to its size — declines instead of under-reporting. The
intrinsic-call meter remains interpreter-owned and still refuses native dispatch;
the Provider-call meter does not need to, because Provider and async operations
remain continuation barriers rather than being hidden in machine code, so the
interpreter performs and charges every Provider call. "Accounting parity status"
below is the per-fact status and the remaining gap list.

## Accounting parity status

The first JIT goal in [../roadmap.md](../roadmap.md) is that native execution
report the same deterministic step, allocation, cancellation, and deadline facts
as the interpreter, so bounded and isolated execution can use it. This section is
the current status of that goal, fact by fact, and the concrete list of what is
still missing. It is deliberately written as status rather than intent: where a
fact cannot be attributed exactly, the native tier declines and the interpreter
runs the region, and that decline is part of the contract rather than a bug.

The interpreter is the accounting oracle. `RegVm::tick`
(`crates/rsscript-vm/src/reg_vm/exec.rs`) charges one source step *before* each
bytecode instruction executes, so an instruction that fails is still counted;
`RegVm::charge_work` charges additional units for work hidden behind one
instruction; and `RegVm::usage` publishes `steps_consumed`,
`allocation_bytes_consumed`, and the live-memory figures into `ExecutionUsage`.

### Step count

**Equivalent** for whole-function regions, OSR regions, and continuations
whenever native code runs at all, with or without a step, cancellation, or
deadline control armed. A program run with step limit `N` terminates with the
same reason and reports the same `steps_consumed` under native execution as under
the interpreter, and a program run with nothing armed reports the interpreter's
count rather than zero. Source-step accounting is therefore unconditional; only
the *rejection* half is conditional. Six mechanisms make that exact:

- **Counting is separated from the ceiling.** `RegionCompileControls::step` asks
  generated code to count; `step_ceiling` asks it to additionally reject. A run
  with no step budget gets the first without the second, so it pays no
  per-segment compare and mints no reservation bail site.
- **Segment reservation** (ceiling armed). Generated code reserves a whole
  no-deopt accounting segment's source cost before the segment's first
  instruction (`step_segment_costs` in
  `crates/rsscript-jit-cranelift/src/codegen.rs`). A segment is capped at 512
  source steps and never crosses a CFG leader, a possibly-bailing instruction, or
  an inlined-region boundary.
- **Block charging** (counting only). With no ceiling to reserve against and no
  cancellation or deadline to poll, a region charges each basic block's whole
  source cost once at its leader (`step_block_costs`), and each bail site
  publishes its own exact resume count on its cold edge by subtracting a
  compile-time constant. The hot path then pays one add per block instead of a
  reservation, a compare, and a live `steps_resume` variable per possibly-deopting
  instruction. That constant exists because an accounting segment never crosses a
  CFG leader; a region whose inlined run reaches back past its block leader has no
  such constant and keeps the segment model.
- **Inlined callee bodies own their own steps.** The leaf inliner returns a
  per-instruction `NativeInlineAccounting`
  (`crates/rsscript-vm/src/reg_vm/native/passes/inlining.rs`), so each spliced
  callee instruction owns one interpreter step and the call itself owns one,
  instead of the whole callee body collapsing onto the caller's call ip and
  owning nothing.
- **All-or-nothing roll-back for an inlined region.** A deopt inside an inlined
  region resumes the interpreter at the caller's call instruction, which
  re-executes the whole call, so generated code reports `steps_resume` — the
  count as of the last precisely-resumable segment entry — rather than the
  running count. A guard bail elsewhere likewise excludes the pre-charged cost of
  the one instruction the interpreter is about to execute again.
- **A native-to-native edge shares one limits cell.** The callee is compiled with
  the caller's `RegionCompileControls`, the caller flushes its running count to
  the call-owned `[steps, step_budget, cancel_addr]` cell before the edge, and the
  callee charges its own source cost on top of it
  (`JitInstr::CallNative` lowering in
  `crates/rsscript-jit-cranelift/src/codegen.rs`). `CallNative` is never
  `step_batch_safe`, so it always ends its accounting segment and the caller's
  `steps_resume` holds the count as of the instruction before the call: a bail
  anywhere in the chain funnels through the caller's `fallback`, whose write-back
  rolls the callee's charge back for the interpreter to re-execute the whole call.
  An armed callee never gets the frame-free direct scalar entry, which carries no
  limits pointer, and `NativeModule::resolve_native_callees` refuses an edge whose
  caller and callee disagree on the controls.
- **Constant key-hash work is billed.** `map_key_from_value`
  (`crates/rsscript-vm/src/reg_vm/value_ops.rs`) bills one unit for a scalar map
  key on top of the instruction's own tick, so an `Int`-keyed map insert, get, or
  membership test costs the interpreter two source steps.
  `charge_native_key_hash_work`
  (`crates/rsscript-vm/src/reg_vm/native/translate/jit_post.rs`) bills that
  constant onto the owning native item. Sorted maps and sorted sets are
  list-backed, hash nothing, and are deliberately absent from that set.

Two shapes **decline** instead of running natively while a control is armed,
because their source cost is not attributable:

- an OSR region whose loop body contains a call the inliner would dissolve
  (`RegVm::build_osr_plan`);
- a region that hashes a key whose cost is proportional to its size
  (`native_source_cost_is_static`), including one reached over a call edge.

A whole-region hand-back that is **not** a precise resume (a failed heap commit,
an unresolvable handle, a mismatched outcome, or a deopt whose precise resume
cannot be reconstructed) makes the interpreter re-run the function from its first
instruction, so `RegVm::attempt_native` restores the pre-entry count on those
exits rather than reporting the region's charge twice. `RegVm::try_osr` does the
same for every exit that leaves the interpreter frame untouched, which makes the
interpreter re-run the loop from its header. For the same reason an
accounted region takes the plain precise resume at its call instruction rather
than reconstructing a child frame through `try_resume_native_child_deopt_chain`:
generated code publishes one roll-back point per region, and for a metered edge
that point is the caller's call instruction.

### Allocation bytes

**Equivalent where admitted, otherwise declined.** The interpreter accounts
through `RegVm::account_bytes` / `ensure_memory_available`
(`crates/rsscript-vm/src/reg_vm/exec/storage_accounting.rs`). Whole-function
native entry admits an armed `allocation_budget` or `live_memory_limit` only with
a per-region read-only proof (`whole_function_memory_controls_supported` in
`crates/rsscript-vm/src/reg_vm/tier.rs`); OSR additionally admits
shape-preserving stores and `List.push`, whose helper charges the exact capacity
delta into a transaction-local cell that is committed only with the heap
transaction. Every other allocating or replacing helper and every native-call
edge fails closed.

### Cancellation

**Termination reason equivalent; observation point is a polling approximation on
both sides.** The interpreter polls `CancellationToken` on the first instruction
and then every `CANCEL_POLL_INTERVAL` (1024) source steps. Generated code loads
the same `AtomicBool` (`CancellationToken::as_atomic`) at every accounting
segment entry and every loop backedge — at most every 512 source steps. Both are
throttled polls, so the exact step at which cancellation is *observed* is not a
deterministic fact on either engine and is not claimed to be equivalent; the
reported reason, and the step count actually paid for by the region before the
bail, are.

### Deadline

**Termination reason equivalent; observation point bounded by the segment.** The
interpreter evaluates `MonotonicDeadline::is_expired` on every tick and in
`charge_work`. Generated code calls the VM's `rss_jit_deadline_expired` helper
(`crates/rsscript-vm/src/reg_vm/jit_native_a.rs`) at the same polling points as
cancellation, before reserving the next segment, so a deopt resumes at the first
unpaid source instruction and the interpreter reports the canonical typed
failure. Deadline latency inside a native region is therefore bounded by one
accounting segment rather than by one instruction.

### Remaining gap list

1. **OSR origins carry no inline accounting.** `native_jit_origins`
   (`crates/rsscript-vm/src/reg_vm/native/translate/jit_post.rs`) derives cost
   from `source_ip` uniqueness, and the OSR pass chain in `RegVm::build_osr_plan`
   (`crates/rsscript-vm/src/reg_vm/tier/osr_plan_builder.rs`) composes ip maps
   across passes without a cost vector. Needed: thread `NativeInlineAccounting`
   through that chain the way `NativePipelineState` already does for
   whole-function translation. Until then **any** OSR region containing a call
   declines. Note the widened blast radius: accounting is now unconditional, so
   this decline, which used to apply only under an armed control, applies to every
   run, and an OSR loop containing a dissolvable call no longer reaches generated
   code at all.
2. **Data-dependent key hashing cannot be charged from generated code.**
   `map_key_from_value` bills `1 + len / 64` for a String/Bytes key and recurses
   for a structural one. The region's step counter is a Cranelift register
   variable, so a host helper cannot add to it. Needed: a limits-cell ABI a
   helper can charge against. Until then `native_source_cost_is_static` declines
   `MapInsertHandleKeyInt` and `SetInsertHandle` regions. That decline is likewise
   now unconditional rather than armed-only, for the same reason as gap 1.
3. **Closure sinking drops the deleted instruction's step.**
   `native_inline_leaf_calls_inner` emits nothing for `sinkable.dead_defs`, so a
   sunk `MakeClosure` and its dead copy `Move`s own no source cost even though
   the interpreter ticks them. Needed: attribute each empty span's cost to the
   next emitted item, or decline when a span is empty and a control is armed.
4. **The intrinsic-call meter stays interpreter-owned.**
   `RegVm::native_preemption_controls_supported`
   (`crates/rsscript-vm/src/reg_vm/exec.rs`) refuses whole-function and OSR
   dispatch whenever `intrinsic_call_budget` is armed, and
   `ExecutionUsage::intrinsic_calls` is likewise under-reported for a natively
   executed region even with nothing armed. The interpreter charges one call in
   `RegVm::charge_intrinsic_call`; generated code runs the same intrinsics as
   host helpers *and* as direct lowerings with no helper at all
   (`ListLenDirect`, the direct flat-list accesses), so there is no single choke
   point a helper could charge from. Needed: a second per-item cost alongside
   `NativeInstructionOrigin::source_cost`, charged the way source steps now are —
   per block with a per-site constant correction — plus a second counter word in
   the limits cell. A host-helper-side charge would be unsound because it would
   miss every directly lowered intrinsic.

   The Provider half of this gate is gone. `RegInstr::CallExternal` is
   `NativeLoweringClass::Yield { ExternalCall }`: it is never lowered into machine
   code, `controlled_static_inline_candidate` refuses a leaf whose effects set
   `may_call_provider`, and a compiled callee must be a scalar leaf whose every
   instruction lowers natively. Generated code therefore cannot reach a Provider,
   and the interpreter performs and charges every Provider call at the barrier, so
   an armed `provider_call_budget` no longer refuses native dispatch.
5. **A custom `max_depth` refuses whole-function native entry.** The internal ABI
   carries a host-stack cap, not the language's logical frame limit
   (`RegVm::attempt_native`). OSR entry already forwards the configured limit.

Gaps 4 and 5 together are why `rss run --trusted-in-process --native`
(`crates/rsscript-cli/src/cli/runner.rs`) still replaces the runner limits with
`RunLimits::unbounded_for_trusted_host()` rather than keeping the default runner
profile. That profile arms `intrinsic_call_budget: 1_000_000` and sets
`max_depth: 256` against the VM's `DEFAULT_MAX_DEPTH` of `16_384`, so keeping it
would refuse every whole-function and OSR region and leave `--native` with no
native tier at all. Closing gap 4 and forwarding the logical depth limit are the
prerequisites for that CLI change; the step budget itself is already exact.

### Measured cost of unconditional accounting

Release gate
(`cargo test --locked --release -p rsscript-sdk --features native-jit --test
native_jit_smoke native_hot_loop_release_gate_beats_the_interpreter`), native
median over interleaved paired runs on one machine, against the same tree with
accounting armed only when a control is armed:

| accounting shape | overhead on the gate |
| --- | --- |
| count and reserve every segment, always | +81% |
| count every segment, reserve only when armed | +17% |
| charge per block, reserve only when armed | +3.6% |

Only the third shape ships. The first two are recorded because they are the
obvious implementations and both miss the gate's ~10% budget; what costs is the
per-segment reservation bail site and the live `steps_resume` variable, not the
counting.

### Coverage this is verified by

`crates/rsscript-sdk/tests/native_jit_differential.rs`:
`native_step_accounting_matches_the_interpreter_under_an_armed_budget`,
`native_step_accounting_matches_the_interpreter_for_osr_entered_loops`,
`native_step_accounting_is_exact_when_a_guard_deopts`,
`cancellation_and_deadline_stop_native_execution_with_the_interpreter_reason`,
and `native_kernel_corpus_reports_the_interpreter_step_count_under_a_budget`,
which replays the whole native kernel corpus under armed step budgets. The first
two replay `STEP_PARITY_CASES`, whose `callee-owns-the-loop`,
`nested-compiled-callee`, and `deopt-inside-compiled-callee` shapes hold a loop
the leaf inliner refuses to dissolve and therefore reach generated code only over
a compiled native-to-native edge.

`an_unbounded_native_run_reports_the_interpreter_step_count` covers the
no-control case with the production tiering defaults, where a hot loop reaches
generated code mostly through OSR.
`an_armed_provider_call_budget_no_longer_refuses_native_dispatch` runs an
in-memory Provider both under and over an armed budget and pins that
whole-function or OSR dispatch happens, which continuation entries alone would
not have shown.

`crates/rsscript-jit-cranelift/src/tests/calls_and_abi.rs` pins the edge ABI
directly: `armed_native_to_native_edge_shares_and_rolls_back_the_step_cell` and
`a_native_call_edge_requires_matching_generated_code_controls`.

## Stable invariants

- The interpreter is the semantic oracle. Unsupported operations and guarded
  failures fall back without committing partial VM or mutable-buffer state.
- Only validated JIT functions reach code generation.
- Diagnostic compile telemetry separates VM translation, sealed validation,
  Cranelift code generation, and finalization. The total compile timer remains the
  admission wall clock and can therefore include orchestration between phases.
- Whole-function, OSR, and continuation lowering converge on
  `NativeRegion<Lowered> -> NativeRegion<Analyzed> -> ValidatedNativeRegion ->
  PublishedNativeRegion`. The sealed validation proof borrows immutable IR, so a
  caller cannot mutate a region between validation and publication.
- Executable memory is reserved from a shared hard budget before code generation.
- Finalized functions follow compile-once-publish: a completed function is made
  reachable, and crossing a soft admission limit closes later compilation.
- Non-tail native recursion is not supported; recursive call graphs run on the
  interpreter.
- Mutable flat-buffer arguments require one unique proof per ABI entry. A mutable
  proof cannot authorize a read-only or second mutable entry.
- Process environment variables do not configure library behavior. Hosts pass
  typed `NativeJitOptions`; diagnostic front ends may translate their own flags.
- Every VM-to-native entry crosses the versioned `JitCallFrame` ABI. The frame
  owns bail, safepoint, deoptimization, depth, limit, and host-context state.
- `NativeModule` owns generated code and immutable deopt metadata only. Mutable
  payload scratch is owned by a reusable `NativeCallSession` in the evaluation;
  two sessions cannot observe each other's normal-yield or deopt payload.
- The native engine is a 64-bit-only component. Compilation fails explicitly on
  other pointer widths; opaque host-context and flat-buffer pointer bits must not
  be truncated into the language's `i64` transport words.
- Every official `extern "C"` Host Helper is a no-unwind trampoline. A Rust panic
  is caught before it can cross generated code, marks the ordinary bail flag, and
  returns the helper type's zero/default value; the VM then aborts the transaction
  and resumes through the interpreter contract.
- A native-to-native edge may use the private frame-free scalar ABI only when the
  callee is a bounded, non-recursive leaf over `Int`/`Bool`/`Float`, every
  reachable instruction is proven unable to deopt, allocate, call a helper,
  suspend, touch a resource, or invoke another function, **and** no generated-code
  limit control is armed: that ABI carries no limits pointer, so an accounted
  callee keeps the versioned child frame and charges the caller's limits cell. Checked integer
  arithmetic and shifts therefore retain the full child-frame path. Direct
  entries are emitted lazily only for compiled callees; ordinary top-level VM
  entries do not duplicate machine code. This internal ABI is process-local and
  carries no independent compatibility promise.
- Reentrant native entry is unsupported and returns the typed
  `NativeDeclineReason::ReentrantCall`; it is never presented as a resumable
  generated-code safepoint.
- Register definitions, register uses, control-flow shape, heap visibility,
  deoptimization, and OSR eligibility are classified by the exhaustive
  `JitInstr::effects` API. Validators and tiering must consume those facts rather
  than maintain independent opcode lists.
- VM bytecode eligibility is expressed as `Direct`, bounded synchronous `Helper`,
  normal `Yield` barrier, or `Reject`, rather than as an unstructured boolean.
  Native telemetry reports dynamically interpreted native-capable work and stable
  barrier-reason counts. These observations decide which continuation regions are
  worth implementing; they do not change execution semantics. Because dynamic
  missed-work classification runs on the interpreter hot path, it is collected
  only under the explicit `NativeCostModel::Report` diagnostic mode. Ordinary
  production execution defaults detailed telemetry off, and the default enforcing
  cost model does not pay that per-instruction cost. `NativeJitOptions::diagnostic`
  or `with_telemetry` is the explicit opt-in for timings and counters.
- Branch and dynamic-call feedback is not collected. The profile-guided
  speculation surface was removed after its controlled workloads failed the
  retention threshold, so the engine holds no profile maps or counters and
  performs no feedback write on interpreted branches or calls.
- Production OSR distinguishes threshold-driven automatic triggering from eager
  first-header triggering. Automatic OSR is enabled by default; eager OSR is off
  by default and reserved for explicit differential or diagnostic execution.
- ABI v3 distinguishes a planned continuation `Yield` from `Deopt`. A yield
  commits completed region work, materializes its bounded live scalar state, and
  resumes the VM at the barrier instruction. A deopt aborts transactional work
  and follows the existing precise-resume or replay contract. The initial stable
  continuation slice admits bounded scalar CFG regions with branches, loops, and
  multiple normal exits around non-`mut` `CallKnown` barriers and function
  returns. Heap values, async, Provider, and resource barriers remain
  interpreter-owned; after a barrier completes, the VM may enter a later scalar
  continuation. Unused heap registers may
  coexist in the VM frame: continuation marshalling validates only the exact
  register footprint of the selected scalar region, so scalar work after an
  interpreter-materialized aggregate can re-enter native code safely.
- Verified-bytecode continuation lowering attaches source-resume liveness to each
  generated guard. JIT validation unions those facts with local JIT liveness and
  intersects them with definite assignment. Dead historical temporaries therefore
  do not inflate state maps, while detached JIT clients that do not provide source
  facts retain the conservative all-assigned behavior.
- Provider calls and `await` are exercised as normal mixed-mode boundaries by
  interpreter/native differential tests. The VM executes each boundary exactly
  once, preserves Provider traces and scheduler semantics, then probes the next
  scalar continuation. Generated code never re-enters the interpreter or spans a
  suspension.
- Every native region meters its source instructions, armed or not, because
  `steps_consumed` is a reported usage fact and not only a ceiling. That includes
  the instructions of a callee the leaf inliner spliced in, the body of a callee
  reached over a native-to-native edge, and the constant key-hash unit the
  interpreter bills on top of an `Int`-keyed map operation. A missing step ceiling
  is represented as `i64::MAX` in the call-owned limits cell and suppresses the
  per-segment reservation entirely rather than being compared against.
  Scheduler-owned async bookkeeping remains outside the native source map.
- Region formation requires at least sixteen direct source instructions. Under the
  enforcing cost model, acyclic dispatch requires at least 512 instructions to
  amortize the trampoline; diagnostic/off modes can still exercise smaller
  correctness fixtures. Closed native loops may yield once at a forward barrier,
  while any backedge to a VM barrier is rejected so execution cannot ping-pong
  across the ABI once per iteration. The canonical aggregate-boundary workload is
  the retention gate for this closed-loop shape.
- Region formation produces evaluation-local facts (included CFG instructions,
  exits, active-register footprint, and exact source work) once. They are cached
  by verified function/IP behind an `Rc`; runtime shape specialization consumes
  those facts without rescanning bytecode, while the persistent Artifact remains
  unchanged.
- Optional typed-executable-facts schema v2 preserves semantic generic parameter
  identities and concrete arguments as ordered call-site pairs without changing
  the bytecode-v1 instruction stream. The verifier recursively checks that pair
  mapping against the executable generic signature and caller register facts.
  Native instance keys include the ordered arguments plus verified concrete
  parameter/result storage and remain capped per function. These lowering-attested
  identities may select a cache instance and seed scalar parameter storage; they
  never authorize nominal layouts, pointer representations, or unsafe lowering.
  Typed-facts v1 remains readable and simply falls back to unavailable generic
  specialization.
- Structural compilation work is bounded by `JitLimits` before Cranelift code
  generation. Instruction, register, CFG-edge, operand, analysis-word, deopt,
  memo-scope, callee, and recursive-group counts have deterministic limits.
  `max_compile_millis` is a soft admission/telemetry limit, not a claim that an
  in-process Cranelift invocation can be interrupted at a wall-clock deadline.

## Internal contracts

`JitFunction` and `JitInstr` are in-process implementation types. They are not
serialized artifacts and do not have a compatibility version independent from the
VM/JIT release. The Artifact and bytecode compatibility contracts remain the only
persistent executable formats.

Optimization passes may not change observable outcomes, output, Provider calls,
heap-visible mutations, cleanup, budget accounting, or deoptimization resume
state. Differential tests against the interpreter enforce these invariants.

Profile-guided closure PIC and branch-side-exit speculation were removed after
their controlled workloads failed to demonstrate a repeatable end-to-end benefit
under the retention rule; they are no longer part of the engine.
The evaluation-local function-state table owns the program identity once and uses
stable ordinals to index a dense function-state vector; program digests are not
cloned into per-function lookup keys.

The stable native path contains a deliberately narrow read-only LICM subset. Its
lazy loop-activation memoization is emitted only when verifier-bound typed facts
produce flow-sensitive ownership/alias evidence, every operand is invariant, and
the existing heap-provenance scan proves that no overlapping write occurs. The
cache instruction is owned exclusively by this production proof; there is no
separate compatibility feature or promotion surface. OSR selection and
helper-hoisting consume
one canonical loop-fact
projection: unique preheader (when present), header condition, latches, exits, and
a conservative affine induction variable. The existing backend range proof may
remove an individual flat-list bounds check, and reports the exact eliminated-site
count separately from checks retained.

The backend also recognizes one deliberately narrow bounds-check-elimination
shape: a non-negative induction variable, an invariant direct-list length bound,
a strict `<` guard, a single `+1` update, and a single-entry/single-exit CFG. It
rejects mutable step/bound/base registers, non-unit steps, non-strict guards, and
analysis that exceeds a checked linear work allowance. This is a proof for an
individual direct flat-list access, not a general range-analysis claim. Backend
tests assert emitted and eliminated-site counts separately, while the controlled
scorecard exposes both counters for workloads that reach the flat-buffer ABI.

Read-only LICM is descriptor-driven rather than a helper-name allowlist. A helper
must be read-only, every operand must be loop invariant, and every declared heap
projection must remain unmodified for the loop activation. Missing or multi-root
projection metadata fails closed. Field-slot reads additionally retain their exact
slot proof. Handle receivers additionally require a program-point alias class of
immutable, uniquely owned, unique-borrowed, or read-borrowed. Unknown, shared, an
over-budget ownership analysis, or a typed-facts disagreement retains the ordinary
helper call.

Scalar x2 unrolling is not enabled. The VM reports only research candidates that
have one latch and exit, a unit constant induction step, at most twelve direct
scalar instructions, and no internal control-flow or effectful helper. Even those
candidates remain unchanged until exact source-step charging, overflow/deopt
resume identity, remainder semantics, differential coverage, and a controlled
end-to-end benchmark all pass. SIMD remains out of scope.

The executable research gate nevertheless reports SIMD candidates: canonical
single-latch, forward unit-stride, read-only list scans with no effectful or
internal control-flow instruction. Mutable scans decline until an injective alias
proof exists. A candidate counter does not enable vector codegen. Automatic
promotion additionally requires verified numeric lane types, bounds/range proof,
exact checked-overflow and deopt lane identity, scalar remainder parity, at least
twenty controlled samples, and a repeatable 15% end-to-end gain. The scorecard
prints both scalar-unroll and SIMD candidate counts so a missing implementation
or missing workload is visible rather than reported as a speedup.

Closure speculation was removed after its controlled scorecard workloads
(`profile-closure-pic`, `profile-branch-cold`) failed to clear the retention
threshold; it is no longer a runnable surface.

Nested and loop-carried struct scalar replacement was likewise removed: the
canonical `native-struct-sr` workload ran net-negative against the interpreter,
and the native path already left those aggregates unchanged and fell closed to
the verified interpreter.

The Cranelift engine crate is not independently published. Its public root exposes
only the VM-facing engine, validated IR, typed options/outcomes, host-helper
contract, and prepared-call boundary. Raw call-frame layout, ABI offsets, helper
function aliases, codegen internals, and module implementation types remain
crate-private.

Native rewrites carry
`NativeInstructionOrigin { source_ip, resume_ip, source_cost, inlined }` in one
owned pipeline state. `inlined` marks an item spliced in from a callee body: it
owns real interpreter steps that have no distinct position in the caller's
bytecode, so it is exempt from the one-cost-per-`source_ip` validation rule and
its charge is rolled back rather than reported when a guard inside the region
bails. JIT instruction indices are CFG identities only. A pass may
temporarily return a local new-to-previous map, but only the pipeline state
composes it; source identity, interpreter resume, and accounting ownership may not
travel in unrelated parallel vectors. Expansion assigns one source cost to exactly
one generated item, fusion preserves the summed cost, and a rewrite that loses
cost is rejected before codegen. Codegen reserves explicit source cost by segment;
deopt maps expose the explicit source/resume positions to the VM.

## Native recursion

Tail recursion may be lowered to a loop. Non-tail self or group recursion would
use the host stack and is not supported: the native-recursion surface was removed
because its only stack boundary was a static frame estimate (an admission
heuristic, not a hard safety proof). Recursive call graphs run on the interpreter.
Re-introduction would require an explicit frame stack, a trampoline, or a
target-backed live stack-limit check.

## Telemetry

Native reports distinguish resident, published, rejected-resident, and reserved
arena bytes. Under compile-once-publish, rejected-resident bytes must remain zero.
They also expose direct-list check sites/elisions and read-only LICM/helper sites,
so a scorecard can distinguish actual optimization from mere compilation.

## Hardening gate

The weekly hardening workflow runs interpreter/native differential tests, forced
deoptimization and rollback cases, 64/128/256 KiB host-stack entries, guard-page
flat-buffer bounds tests, AddressSanitizer coverage for host wrappers, structured
IR fuzzing, and the workload scorecard. ASan does not instrument generated machine
code; guard pages and canary/boundary fixtures cover direct native memory accesses.

The stable retention set is baseline scalar/flat-data execution, native leaf-call
chains, transactional helpers, precise deopt, and the Option/Result/Variant scalar
replacement paths that enter on the canonical scorecard. Speculation, non-tail
native recursion, and struct scalar replacement were removed after failing the
retention rule; the alias-gated read-only LICM subset is retained in production. A local scorecard
run is diagnostic only; timings become a compatibility or release signal only
after a controlled-hardware baseline is checked in with machine/toolchain metadata.
