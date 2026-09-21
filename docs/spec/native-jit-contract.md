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
attributed exactly declines instead of under-reporting, but no shape is declined
structurally any more. An OSR region composes the same owned cost vector the
whole-function pipeline does across its whole pass chain, so a loop containing a
call the inliner dissolves is accounted rather than declined, and a rewrite that
loses a charge declines that region; a key whose hash work is proportional to its
size is charged at runtime by the helper that hashes it, against the same
call-owned cell. The
intrinsic-call meter is charged the same way: every native item carries an
explicit intrinsic cost beside its source cost, because generated code runs the
same intrinsics as host helpers *and* as direct lowerings and there is no single
helper-side choke point. The Provider-call meter needs neither, because Provider
and async operations remain continuation barriers rather than being hidden in
machine code, so the interpreter performs and charges every Provider call.
"Accounting parity status" below is the per-fact status and the remaining gap
list.

## Accounting parity status

The first JIT goal in [../roadmap.md](../roadmap.md) is that native execution
report the same deterministic step, intrinsic-call, allocation, cancellation, and
deadline facts as the interpreter, so bounded and isolated execution can use it. This section is
the current status of that goal, fact by fact, and the concrete list of what is
still missing. It is deliberately written as status rather than intent: where a
fact cannot be attributed exactly, the native tier declines and the interpreter
runs the region, and that decline is part of the contract rather than a bug.

The interpreter is the accounting oracle. `RegVm::tick`
(`crates/rsscript-vm/src/reg_vm/exec.rs`) charges one source step *before* each
bytecode instruction executes, so an instruction that fails is still counted;
`RegVm::charge_work` charges additional units for work hidden behind one
instruction; `RegVm::charge_intrinsic_call` charges one intrinsic dispatch; and
`RegVm::usage` publishes `steps_consumed`, `intrinsic_calls`,
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
- **The OSR pass chain carries the same cost vector.** `RegVm::build_osr_plan`
  (`crates/rsscript-vm/src/reg_vm/tier/osr_plan_builder.rs`) seeds
  `native_osr_seed_origins` from the leaf inliner's `NativeInlineAccounting` and
  composes every rewrite hop onto it with `native_compose_origins`
  (`crates/rsscript-vm/src/reg_vm/native/translate/jit_post.rs`) — the same rule
  `NativePipelineState::apply_rewrite` applies to the whole-function chain: only
  the first item mapping to a predecessor re-charges it, and a hop whose total
  source or intrinsic cost changed is rejected, so a pass that deleted a source
  instruction without moving its charge onto a surviving item declines the region.
  All eleven hops are composed, including the two whose maps used to be computed
  and dropped (`native_elide_readonly_full_list_slices_in_region` and
  `native_lower_checked_payload_intrinsics_in_region`, both index-identity today —
  a future pass that stopped being index-identity is now caught rather than
  ignored). The seed is re-based through the combinator-expansion map, because
  that pass runs *before* inlining: one real `Option.map`/`Result.unwrap_or`
  `CallIntrinsic` owns one interpreter tick and one intrinsic dispatch however
  many expanded items it became, while the mapper body the inliner splices in
  owns its instructions one by one, exactly as `RegVm::call_closure_one` ->
  `run_frame` ticks them. The resulting vector reaches `native_jit_origins`
  through `OsrTranslationRequest::source_accounting`; the two rewrites inside
  `translate_osr_loop_inner`
  (`native_memoize_loop_invariant_runtime_helper_calls`,
  `native_forward_direct_list_store_loads`) take `&mut [JitInstr]` and can neither
  add nor remove an item, so they are index-stable by construction.
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
- **Data-dependent key-hash work is charged by the helper that hashes.** A
  `String`/`Bytes` map or set key costs the interpreter `1 + len / 64`, which is a
  property of the runtime value and has no compile-time constant an item could
  carry. The helper is the only place the size is known and generated code keeps
  its running count in an SSA variable, so the region publishes that count to the
  limits cell before the call and adopts the helper's result afterwards — the same
  flush/reload a native-to-native call edge uses. `rss_jit_map_insert_handle_key_int`
  and `rss_jit_set_insert_handle` resolve the key through the interpreter's own
  `map_key_from_value`, so the work units and the hashability verdict are the
  host's, then call `vm_jit::charge_hidden_work` **before** the journaled write,
  exactly where `RegVm::charge_work` sits. When an armed step budget no longer
  fits, the helper signals a bail without writing: the region's transaction rolls
  back, `steps_resume` holds the count as of the instruction before it (the call
  can bail, so it ends its accounting segment), and the interpreter re-executes the
  instruction, charges the same units and raises the canonical
  `StepBudgetExceeded`. Codegen emits the flush/reload only when the region's
  controls say `step_ceiling || cancel || deadline`, which is precisely when
  `charge_work` charges anything at all, so an unarmed run bills nothing on either
  engine. `native_source_cost_is_static` reads the "this helper charges its own
  work" fact from `HostHelper::charges_data_dependent_work` rather than restating
  the helper list, so a data-dependent helper added on one side and not the other
  declines the region instead of running uncharged.
- **Constant key-hash work is billed on exactly the interpreter's condition.**
  `map_key_from_value` (`crates/rsscript-vm/src/reg_vm/value_ops.rs`) bills one
  unit for a scalar map key on top of the instruction's own tick, so an
  `Int`-keyed map insert, get, or membership test costs the interpreter two
  source steps. It bills it through `RegVm::charge_work`, which charges nothing
  at all when no step budget, cancellation token, or deadline is armed.
  `charge_native_key_hash_work`
  (`crates/rsscript-vm/src/reg_vm/native/translate/jit_post.rs`) bills that
  constant onto the owning native item under the same condition, which the
  region's `RegionCompileControls` carry and the compiled version key
  distinguishes. Applying it unconditionally made an unarmed OSR'd map loop
  *over*-report one step per hashing instruction. Sorted maps and sorted sets are
  list-backed, hash nothing, and are deliberately absent from that set.

No shape **declines structurally** for accounting reasons any more. What remains
is conditional: a region declines when its own pass chain loses a charge at some
hop, when a transformed item cannot be traced back to a real bytecode instruction,
or when it contains a data-dependent site whose helper does not charge its own
work (`native_source_cost_is_static`). Accounting is unconditional, so those
declines apply to every run rather than only to an armed one.

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

### Intrinsic calls

**Equivalent** for whole-function regions, OSR regions, and continuations
whenever native code runs at all, with or without an intrinsic-call budget armed,
and an armed `intrinsic_call_budget` no longer refuses native dispatch.

The interpreter charges one call in `RegVm::charge_intrinsic_call`
(`crates/rsscript-vm/src/reg_vm/exec.rs`), reached from exactly two bytecode
instructions: `RegInstr::CallIntrinsic` and `RegInstr::CallTypedIntrinsic`.
Generated code runs those intrinsics both as host helpers *and* as direct
lowerings with no helper at all (`ListLenDirect`, the direct flat-list accesses),
so a helper-side charge would be unsound. Instead
`JitInstructionOrigin::intrinsic_cost` sits beside `source_cost` and is derived
the same way: the item that owns a source instruction's step also owns its
intrinsic dispatch, a spliced callee instruction owns its own, and a dispatch that
owns no source step fails the pipeline closed rather than running uncharged.

The cost is read from the **source** instruction, not the transformed one the
item lowers. A region rewrite may replace or delete the dispatch — the string and
bytes length-law folds turn `String.len`/`Bytes.len` into arithmetic on operand
byte lengths and delete the now-dead allocation — while the interpreter still
runs the intrinsic and bills it.

Codegen charges the meter at the same points as source steps: once per basic
block at its leader for a counting-only region, once per accounting segment at
its entry otherwise, corrected on each cold bail edge by the same compile-time
constant, and flushed/reloaded across a native-to-native call edge. The
call-owned limits cell is `[steps, step_budget, cancel_addr, intrinsic_calls,
intrinsic_budget]`, and a missing ceiling is `i64::MAX`. A region that owns no
intrinsic dispatch materializes no counter at all, so it emits byte-identical
code. When an armed budget does not fit, generated code leaves the count
untouched and resumes the interpreter at the first uncharged source instruction,
where `charge_intrinsic_call` raises the canonical `IntrinsicBudgetExceeded`.

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
edge fails closed — including the string helper an OSR-lowered
`String.concat` calls, so a loop that builds its own map key runs natively with
no memory control armed and declines with one armed.

The proof is per region, and every region is offered it. A function that declines
because it contains a call does not take its callees down with it: the
interpreter runs the calling function, pushes a real frame per call, and each
callee is admitted or declined on its own proof. `fn main() { ... hot(200000)
... }` under `RunnerLimitsV1::default()` therefore executes the helper natively —
whole-function for a scalar helper, OSR for a `List.push` helper — while `main`
itself stays interpreted. Gap 3 below is what remains: the *calling* region.

### Recursion depth

**Equivalent, or declined.** `RegionCallControls::logical_depth` forwards the
configured `max_depth` into the call frame for whole-function entry as well as
OSR and continuation entry, and `TailCallGuard` enforces it for a tail-recursive
loop. The two other ways a region adds interpreter frames carry no generated
guard: a compiled native-to-native edge costs one frame per chain hop, bounded by
the compiled entry's static `native_call_depth`, and a leaf call the inliner
dissolved costs one further frame, because `controlled_static_inline_candidate`
refuses a callee that itself calls. `RegVm::attempt_native` therefore declines
when the configured limit is within reach of that bound, so the interpreter —
which raises the canonical depth error in `RegVm::push_frame` — owns every run
that could reach the limit. Non-tail recursion is not lowered at all and runs on
the interpreter.

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

1. **Two hand-written kernels still decline, each for a reason of its own.** The
   loop-recognition half of this entry is closed. `detect_natural_loops`
   (`crates/rsscript-vm/src/reg_vm/native/translate/loop_regions.rs`) read a loop
   as the contiguous interval `[header, exit)`, and MIR numbers a `while`'s exit
   block *before* the blocks its body needs — so a body holding a hand-written
   `match` (or any other nested branch) leaves that exit block laid out *between*
   two of the loop's own blocks, `exit` lands below the latch, and recognition
   returned **no loop at all**: `select_osr_candidate_loops` offered nothing,
   `RegVm::try_osr` was never reached, no region was generated and the
   interpreter owned every step exactly.

   The precondition that failed was contiguity, not the number of branches in the
   body. `CanonicalLoopGraph::holed_facts_at` now computes loop membership by
   reachability from the header with the exit edge cut, so a loop with internal
   conditional control flow is recognized whenever it still has one header,
   backedges only to that header, one edge leaving it and no in-loop `Return`.
   The instructions inside the span that are not loop instructions are its
   `hole`, accepted only when the hole starts exactly at the exit and every run
   of it is bounded by terminators on both sides.
   `native_normalize_osr_loop_layout` then relocates the hole past the loop
   before `RegVm::build_osr_plan` runs its pass chain, which is what makes the
   region the contiguous `[header, exit)` every OSR consumer reads.

   That relocation is a **pure permutation**: no instruction is added, removed or
   duplicated, only branch targets are remapped, and the header keeps its source
   ip (only instructions after it move), so the interpreter's header check is
   unchanged. Each source instruction therefore still owns exactly one item and
   exactly one interpreter step, and the permutation is composed onto
   `expand_map` so every later hop — the boundary mapping, the per-item resume
   map and the cost vector — lands directly on the interpreter's own index.
   `detect_natural_loop_at` and `detect_canonical_loops` deliberately keep
   returning contiguous loops only, because read-only LICM and the candidate scan
   both read `[header, exit)` as the whole loop and a partial region would be a
   silent unsoundness rather than a missed optimization.

   `osr_option_loop.rss` now tiers up, as does the `Result` sibling
   (`match checked(v: i) { Ok(value) => ... Err(_) => ... }` over a dissolvable
   leaf call). What remains is the other two kernels this entry used to group
   with them, and measurement shows neither was ever a loop-recognition problem —
   both loops are contiguous and always were recognized:

   - `osr_struct_loop.rss` declines because `p.x` on a locally built struct lowers
     to `GetField` keyed by *name*, which `native_lowering_class` classifies as an
     aggregate barrier. The loop-local struct pass
     (`native_scalar_replace_structs_in_region`) dissolves `MakeStruct` +
     `GetFieldSlot`, and a slot-keyed read exists only inside the typed-region
     lowering the *direct* OSR path derives — which this loop cannot reach,
     because its untyped region is not native-subset. Needed: either a name-keyed
     arm in the struct pass or a slot resolution before native-subset checking.
   - `osr_closure_loop.rss` declines because its closure is a *parameter*: there
     is no `MakeClosure` for the sinking pass to name a callee from, and naming
     one dynamically is the profile-guided closure PIC that was removed after
     failing its retention threshold. This is a deliberate decline, not a gap to
     close.

   Both are pinned as declines by
   `the_kernels_the_layout_normalization_does_not_unblock_still_account_exactly`,
   so a change that does unblock one has to update this list rather than pass
   silently.

2. **A key the loop builds now reaches generated code.** The OSR lowering had no
   arm for a `StringConcat` whose result survives as a live heap `String` — the
   string length-law fold exists to dissolve such a value, not to keep one — so
   `while ... { let key = String.concat(...); Map.insert(map: mut table, key, ...) }`
   was refused at `translate_osr_loop_inner` with `lower reject: StringConcat`
   and the interpreter owned the loop. The arm now lowers it through the same
   host helper the whole-function translator uses
   (`native_string_concat_host`), so the loop runs natively and the key's
   data-dependent hash work is charged by the helper that hashes it, against the
   same call-owned cell.

   **What is covered.** A concat whose own destination is a non-parameter
   register that no instruction outside `[header, exit)` reads. MIR always
   concatenates into a temporary and copies out of it, so a key the loop *carries
   past the backedge and reads after the loop* is covered too: the copy's
   register is an ordinary Handle live-out, which the clean OSR exit materializes
   out of the heap table (`handle_liveouts` in `RegVm::try_osr`) before the heap
   transaction commits, and `osr-built-key-live-after-the-loop.rss` varies the
   key's length per iteration so a stale interpreter slot would change the
   program's output. **What still declines**: a concat writing directly into a
   parameter slot or into a register the interpreter reads after the loop. The
   live-out materialization skips parameters, so that shape fails closed rather
   than leaving a stale value behind; no source spelling produces it today, and
   the arm does not depend on that being true.

   Accounting is unchanged by the arm. `RegInstr::StringConcat` costs the
   interpreter one tick, no hidden work and no intrinsic dispatch, so the item
   owns exactly one source step and no intrinsic call like any other
   copy-through item. **Allocation still fails closed**: the helper allocates
   through the region's heap transaction but does not charge its own capacity
   delta into the transaction-local allocation cell the way `List.push` does, so
   `osr_memory_controls_supported` refuses the whole region whenever an
   allocation budget or a live-memory limit is armed. Closing *that* is the same
   work "Allocation bytes" above describes for every other allocating helper.

3. **A region containing a call still needs a per-region allocation proof.** See
   "Allocation bytes" above: whole-function entry admits an armed
   `allocation_budget` or `live_memory_limit` only for a body that cannot grow
   retained storage, which a function containing *any* call is not.
   `whole_function_memory_controls_supported` is unchanged, and the reason it is
   unchanged is concrete rather than conservative: the interpreter charges a
   called frame's register-window growth through `RegVm::ensure_regs`
   (`crates/rsscript-vm/src/reg_vm/exec.rs`), which bills
   `grew * (size_of::<VmValue>() + 1)` against the high-water mark of the shared
   register stack. That charge is data-dependent on the stack depth at the call,
   so an inlined callee body or a native-to-native edge has no compile-time
   constant to reserve it with, and the region declines rather than
   under-reporting. Threading the memory controls through the call edge the way
   the step and intrinsic cells were threaded would still leave that charge
   unattributed.

   What this no longer costs is the *callee*. A `main` that calls a hot helper
   now runs the helper natively under the default runner profile, on the helper's
   own proof: `main` declines whole-function entry exactly as before, and the
   interpreter's `CallKnown` pushes a real frame for the helper, which
   `RegVm::attempt_native` then admits (a scalar body cannot grow storage) or
   which OSR admits through the `List.push` transaction cell. Nothing tiers "up"
   by call count — `JitState::call_count` is a constant `0` and there is no
   tier-up threshold for whole-function entry, which is offered on every fresh
   frame. What used to hide the helper was the tier-0 executor: `RegVm::run_jit`
   (`crates/rsscript-vm/src/reg_vm/tier/jit_entry.rs`) runs a whole call tree
   inside one frame, executing a `CallKnown` to a pure-leaf callee through
   `run_jit_pure_leaf` instead of pushing a frame, so the callee never re-entered
   `RegVm::drive` and was never offered to the native tier at all. `drive`
   (`crates/rsscript-vm/src/reg_vm/exec_ops.rs`) now keeps such a frame on the
   interpreter loop whenever the native engine is active and
   `JitState::tier0_hides_native_callee` reports that tier-0 would swallow a
   callee that is not yet `NATIVE_STATUS_NOT_ELIGIBLE`. The check reads each
   callee's current status, so a callee whose native attempt reaches an invariant
   decline returns its caller to tier-0. Step and allocation accounting are
   identical across that switch: both paths tick once per source instruction and
   both open the callee window through `prepare_frame` -> `ensure_regs`.

### Closed in the most recent round

These were gap-list entries; they are kept as the record of what was measured and
decided, because both decisions turned on evidence rather than on principle.

- **A data-dependent key's hash work is charged by its helper.** The obstacle was
   never the ABI — the limits cell already existed and was already
   forwarded into every child frame — but that the running count lives in a
   Cranelift register between charge points, so a helper adding to the cell would
   be overwritten by the next write-back. Codegen now brackets the two hashing
   helpers with the same flush/reload `CallNative` uses, `HostCallContext` carries
   the cell so `vm_jit::charge_hidden_work` can reach it, and both helpers bill
   the interpreter's own `map_key_from_value` units before their journaled write.
   `native_source_cost_is_static` no longer declines them. See "Step count" above
   for the full rule.

   Making the decline reachable took one further change, which is a lowering fix
   rather than an accounting one: the OSR type inference forced an unsorted map or
   set key operand to `Int` unless it was a heap *parameter*, which conflicted with
   the producer's own `Handle` typing and declined the whole region — so
   `MapInsertHandleKeyInt` and `SetInsertHandle` were unreachable outside a
   heap-parameter key regardless of accounting. A key operand the region has
   already proven to be a `Handle` now keeps it (`native_key_operand_ty` in
   `crates/rsscript-vm/src/reg_vm/native/translate/osr_loop.rs`).

   A key the loop *builds* was gap 2 above and now lowers too; what its helper
   still cannot do is charge its own allocation, so the region declines whenever
   a memory control is armed.

- **A sunk closure's steps are attributed, and the loop now runs natively.**
   `native_inline_leaf_calls_inner` deletes a sunk `MakeClosure` and its dead copy
   `Move`s and emits nothing for them, so they owned no source step
   while the interpreter ticked them. Each deleted instruction's step now moves
   onto the next emitted item, which is exact only because the pass first proves
   that item cannot run without the deleted one having run: a sunk definition is
   never a terminator, so control always falls through from it, and its successor
   must not be a branch target. A deleted instruction that dispatches an intrinsic,
   a successor that is a branch target, and a deleted span with no surviving item
   after it all fail the pass closed.
   `sunk_instruction_accounting_tests` (`passes/inlining.rs`) pins both halves at
   the pass level.

   With the attribution in place the two gates that made the shape unreachable are
   open. `native_inline_leaf_calls_inner` gained the sunk-`CallClosure` inline arm
   its own comment already described — the callee is known statically from the
   `MakeClosure`, so there is no profile, no identity guard and no dispatch
   sequence, only the capture `Move`s, the argument binds and the spliced body —
   and `native_readable_or_sinkable_closure_operand_candidate` now proves the one
   property the later passes cannot recover: the closure value flows only into
   copy `Move`s and `CallClosure` closure operands, so it never escapes as a value.
   This is **not** the removed profile-guided closure PIC:
   `monomorphic_closure_inline_target` and `polymorphic_closure_inline_targets`
   remain `None`, and nothing here speculates or guards.

   Measured on `benchmarks/vm-jit/kernels/native_closure_sinking.rss`
   (300000 iterations, release, median of five interleaved pairs):

   | build | interpreter | native |
   | --- | --- | --- |
   | before | 71.8 ms | 103.3 ms |
   | sunk-`CallClosure` inline arm only | 73.5 ms | 105.9 ms |
   | arm + OSR closure-operand candidate | 72.1 ms | **1.6 ms** |

   The middle row is why the decision needed the measurement rather than the
   change: with only the inline arm the kernel still generated no region at all —
   `hot` is called once, so whole-function entry never crosses the tier-up
   threshold, and the OSR candidate filter still refused the closure-bearing loop —
   so the native engine kept paying its 44% failed-attempt overhead for nothing.
   Both gates together turn the loop into an allocation-free scalar OSR region
   worth 45x. `steps_consumed` is 7200035 on both engines in all three rows and
   `intrinsic_calls` is 5, so the win costs no accounting parity.

`rss run --trusted-in-process --native`
(`crates/rsscript-cli/src/cli/runner.rs`) now keeps
`runner_limits(&RunnerLimitsV1::default())` - the same profile the interpreter
path runs under - instead of replacing it with
`RunLimits::unbounded_for_trusted_host()`. That profile arms
`intrinsic_call_budget: 1_000_000` and sets `max_depth: 256` against the VM's
`DEFAULT_MAX_DEPTH` of `16_384`, both of which used to refuse every
whole-function and OSR region; neither does now. `--native` selects an
accelerator, not a trust level. A program whose hot loop lives in a called helper
now reaches the native tier under that profile too; what gap 3 still costs is
native entry for the *calling* region, not for the helper.

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

Only the third shape ships. Adding the intrinsic meter on the same charge points
measured within run-to-run noise on the same gate (native median 1.65 ms against
1.61 ms over four paired runs), because the gate's hot loop dispatches no
intrinsic and therefore materializes no second counter at all. Keeping a frame
whose tier-0 run would swallow a native-eligible callee on the interpreter loop
(gap 3) emits no different code and measured 1.83 ms against 1.79 ms over four
interleaved paired runs, within noise on the same gate: the gate's own `main`
was never tier-0 eligible, because its `Output.write` barrier is not a tier-0
instruction. Enabling closure sinking end to end likewise leaves the gate's
own code untouched — its loop allocates no closure — and measured a native median
of 1.76 ms against 2.00 ms over four interleaved paired runs; the shape it does
change, `native_closure_sinking.rss`, is in the closure-sinking table above.
Lowering a `StringConcat` that produces a live heap `String` (gap 2) adds one
match arm the gate's own loop never reaches — it builds no string — and measured
a native median of 1.41 ms against 1.46 ms over four interleaved paired runs.
Relocating a `match`-shaped loop's exit block before the OSR pass chain runs
(gap 1) emits no different code for the gate either — the gate's own loop is
contiguous, so `native_normalize_osr_loop_layout` returns `None` and the chain is
byte-for-byte the previous path — and measured a native median of 1.53 ms against
1.51 ms over four interleaved paired runs, within noise.
Composing the OSR pass chain's cost vector likewise emits no
different code for the gate — whose loop is call-free and takes the direct OSR
entry — and measured a native median of 1.72 ms against 1.79 ms over four
interleaved paired runs. The first shape of that change did cost a measurable
~7% (1.85 ms against 1.73 ms) by allocating a `HashSet` per hop for an
eleven-hop chain that is index-identity at almost every hop; `native_compose_origins`
now returns immediately on an identity map and uses a `Vec<bool>` otherwise,
which is what the paired median above measures. The first two are recorded because they are the
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

`a_closure_bearing_loop_accounts_steps_exactly_in_generated_code` replays a loop
that allocates and calls a `local` closure across the whole `STEP_PARITY_BUDGETS`
list with eager OSR off and on, pinning the interpreter's `steps_consumed` and
`intrinsic_calls` *and* that the loop reaches generated code — the sunk
`MakeClosure` and its dead `Move`s are deleted inside that region, so the counts
are a statement about the attribution rather than about a decline.

`an_unbounded_native_run_reports_the_interpreter_step_count` covers the
no-control case with the production tiering defaults, where a hot loop reaches
generated code mostly through OSR.
`an_armed_provider_call_budget_no_longer_refuses_native_dispatch` runs an
in-memory Provider both under and over an armed budget and pins that
whole-function or OSR dispatch happens, which continuation entries alone would
not have shown.

The intrinsic meter is covered by
`native_intrinsic_accounting_matches_the_interpreter_under_an_armed_budget`,
`..._at_region_boundaries` (a step budget armed alongside, so the region uses the
segment-reservation model rather than block charging),
`..._under_eager_osr`,
`an_unbounded_native_run_reports_the_interpreter_intrinsic_call_count`, and
`an_armed_intrinsic_call_budget_no_longer_refuses_native_dispatch`.
`INTRINSIC_PARITY_CASES` deliberately mixes a direct lowering (`List.len` becomes
`ListLenDirect`, a flat `List.get` becomes a direct load), a read-only host
helper (`String.len`), an `Int`-keyed map get whose constant key-hash unit rides
the step meter beside it, and an intrinsic inside a leaf call the inliner
dissolves, so a helper-side charge would fail three of the four.

`an_osr_loop_that_builds_its_map_key_accounts_exactly_armed_and_unarmed` replays
`OSR_BUILT_KEY_CASES` — a built map key, the same key in a set, and a built key
the loop carries past the backedge and reads after it — unarmed, across the whole
`STEP_PARITY_BUDGETS` list and under an armed intrinsic budget, with eager OSR
off and on, asserting `osr_entries > 0` alongside the interpreter's steps,
intrinsic calls and output. `an_osr_loop_that_builds_its_map_key_declines_under_armed_memory_controls`
pins the other half: with an allocation budget or a live-memory limit armed the
region declines and the interpreter's allocation bytes are reported.

`an_osr_loop_whose_body_matches_reaches_generated_code_and_accounts_exactly` and
`an_osr_loop_whose_body_matches_accounts_intrinsics_under_the_production_defaults`
replay `OSR_MATCH_LAYOUT_PARITY_CASES` — the `osr_option_loop.rss` kernel, a
`Result` match over a dissolvable leaf call, and an `Option` match whose payload
is a heap `String` the scalar replacement cannot dissolve — across the whole
`STEP_PARITY_BUDGETS` list under eager OSR and the production defaults, each
asserting `osr_entries > 0` so neither can pass by declining the region. Their
companion `the_kernels_the_layout_normalization_does_not_unblock_still_account_exactly`
pins `osr_struct_loop.rss` and `osr_closure_loop.rss` as interpreter-owned with
exact counts, naming the reason each one declines.

`native_step_accounting_matches_the_interpreter_for_an_osr_loop_containing_an_inlined_call`
and `an_osr_loop_containing_an_inlined_call_reports_the_interpreter_intrinsic_call_count`
replay `OSR_INLINE_PARITY_CASES` — a dissolvable leaf call in the loop, that call
alongside the string length-law fold, and the Option/Result combinator expansion
with and without a dissolvable `CallKnown` — across the whole
`STEP_PARITY_BUDGETS` list and the armed/unarmed intrinsic budgets, under eager
OSR and under the production tiering defaults. Each asserts `osr_entries > 0` per
case and per `eager_osr` setting, so neither can pass by declining the region.

`a_rewritten_osr_region_reports_the_interpreter_intrinsic_call_count` covers the
regions whose rewrites replace or delete the dispatch; its companion
`the_rewritten_osr_cases_reach_generated_code_outside_whole_function_entry` pins
that those loops are entered through OSR or a continuation rather than through
whole-function translation, which accounts through a different pipeline. Derived
from the transformed stream instead of the source instruction, those three shapes
report 47, 92 and 3 intrinsic calls against the interpreter's 3002, 6002 and
3003.

`a_custom_max_depth_no_longer_refuses_whole_function_native_entry` covers the
logical frame limit, and
`the_default_runner_limit_profile_still_admits_native_dispatch` /
`..._still_stops_an_over_budget_native_run` cover the whole default runner
profile that `rss run --trusted-in-process --native` now keeps.
`a_called_hot_helper_reaches_native_under_the_default_runner_limits` covers the
`fn main() { ... hot(200000) ... }` shape under that profile for a scalar helper
and for a `List.push` helper, pinning `native_calls + osr_entries > 0` alongside
the interpreter's outcome, `steps_consumed`, `intrinsic_calls`, allocation bytes,
peak live memory, and live memory at return.
`a_called_hot_helper_stops_on_the_interpreter_memory_reason` pins the same facts
when the ceiling trips: an allocation budget and a live-memory limit that run out
inside a growing helper *after* a scalar helper has already executed natively
(so the ceiling is enforced against a partly native run rather than by refusing
dispatch), and an allocation budget one byte short of the scalar shape's
completed usage, which must trip on the register-window growth
`RegVm::ensure_regs` charges when the callee's frame is opened.
`crates/rsscript-cli/tests/cli.rs` covers the CLI itself end to end:
`trusted_native_execution_keeps_the_default_runner_step_budget` and
`trusted_native_execution_still_engages_under_the_default_runner_limits`. The CLI
runs with telemetry collection off, so the second pins the native engine and
identical usage rather than a compiled-region count; the differential test above
pins `native_calls + osr_entries > 0` under the same profile.

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
  owns bail, safepoint, deoptimization, depth, limit, and host-context state. Its
  limits word points at a call-owned `[steps, step_budget, cancel_addr,
  intrinsic_calls, intrinsic_budget]` cell shared by the whole native chain.
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
- Every native region meters its source instructions and its intrinsic
  dispatches, armed or not, because `steps_consumed` and `intrinsic_calls` are
  reported usage facts and not only ceilings. That includes the instructions of a
  callee the leaf inliner spliced in, the body of a callee reached over a
  native-to-native edge, and — when the interpreter's own `charge_work` is
  active — both the constant key-hash unit it bills on top of an `Int`-keyed map
  operation and the length-proportional unit it bills for a `String`/`Bytes` key,
  which the hashing helper charges against the same cell while the region flushes
  and reloads its running count around the call. Each meter's missing ceiling is represented as `i64::MAX` in the
  call-owned limits cell and suppresses its per-segment reservation entirely
  rather than being compared against. Scheduler-owned async bookkeeping remains
  outside the native source map.
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
a conservative affine induction variable. A loop whose own blocks MIR did
not lay out contiguously additionally carries the `hole` those blocks leave —
its post-loop block — and is offered to OSR only after
`native_normalize_osr_loop_layout` has relocated that hole past the loop, which
is a pure permutation of the function's instructions with its branch targets
remapped. The existing backend range proof may
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

Profile-guided closure speculation was removed after its controlled scorecard
workloads (`profile-closure-pic`, `profile-branch-cold`) failed to clear the
retention threshold; it is no longer a runnable surface, and
`monomorphic_closure_inline_target` / `polymorphic_closure_inline_targets` remain
`None`. Static closure *sinking* is a different mechanism and is retained: a
`MakeClosure` whose value provably flows only into copy `Move`s and `CallClosure`
closure operands names its callee at compile time, so its allocation is dissolved
and its body spliced with no profile, no identity guard and no dispatch — see gap
3 above for the retention measurement.

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
`NativeInstructionOrigin { source_ip, resume_ip, source_cost, intrinsic_cost,
inlined }` in one owned pipeline state. `intrinsic_cost` is the interpreter's
intrinsic-dispatch charge for the same item and travels with `source_cost`
through every hop: an item that owns no source step may not own an intrinsic
call, and a rewrite that changes either total is rejected. `inlined` marks an item spliced in from a callee body: it
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
chains, transactional helpers, precise deopt, static loop-local closure sinking,
and the Option/Result/Variant scalar replacement paths that enter on the canonical
scorecard. Speculation, non-tail
native recursion, and struct scalar replacement were removed after failing the
retention rule; the alias-gated read-only LICM subset is retained in production. A local scorecard
run is diagnostic only; timings become a compatibility or release signal only
after a controlled-hardware baseline is checked in with machine/toolchain metadata.
