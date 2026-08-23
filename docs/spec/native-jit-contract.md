# Native JIT contract

The native JIT is an optional accelerator for trusted, in-process execution. It
does not add an isolation boundary and never changes Provider authority.

The supported `native-jit` feature is intentionally a bounded feature surface.
Whole-function and transformed OSR entry still decline when execution controls
whose source costs they cannot reproduce are armed. Scalar continuation regions,
however, have a one-to-one source instruction map: acyclic regions account steps
exactly and poll host cancellation. Closed native loops are admitted for trusted
unbounded execution but remain on the interpreter when step or deadline limits are
armed. Because these regions cannot allocate or call
intrinsics/Providers, the surrounding VM barriers continue to own those budgets,
allowing the default bounded profile to accelerate scalar work safely. Armed step
budgets conservatively keep loop regions in the interpreter. Deadline-armed
execution likewise admits only acyclic regions (at most 2,048 source instructions),
then returns to a VM-owned barrier which performs the next monotonic clock poll.

## Stable invariants

- The interpreter is the semantic oracle. Unsupported operations and guarded
  failures fall back without committing partial VM or mutable-buffer state.
- Only validated JIT functions reach code generation.
- Executable memory is reserved from a shared hard budget before code generation.
- Finalized functions follow compile-once-publish: a completed function is made
  reachable, and crossing a soft admission limit closes later compilation.
- Non-tail native recursion is disabled unless the trusted host opts in explicitly.
- Mutable flat-buffer arguments require one unique proof per ABI entry. A mutable
  proof cannot authorize a read-only or second mutable entry.
- Process environment variables do not configure library behavior. Hosts pass
  typed `NativeJitOptions`; diagnostic front ends may translate their own flags.
- Every VM-to-native entry crosses the versioned `JitCallFrame` ABI. The frame
  owns bail, safepoint, deoptimization, depth, limit, and host-context state.
- A native-to-native edge may use the private frame-free scalar ABI only when the
  callee is a bounded, non-recursive leaf over `Int`/`Bool`/`Float` and every
  reachable instruction is proven unable to deopt, allocate, call a helper,
  suspend, touch a resource, or invoke another function. Checked integer
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
  telemetry and the default enforcing cost model do not pay that per-instruction
  cost.
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
- Continuations always meter their one-to-one source instructions, including in
  trusted/unbounded mode. Straight-line execution reports therefore preserve
  interpreter step accounting; scheduler-owned async bookkeeping remains outside
  the native source map. A missing step ceiling is represented as `i64::MAX` in
  the private call cell; it disables rejection without disabling usage
  accounting.
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

Profile-guided closure PIC and branch-side-exit speculation are excluded from the
stable SDK path. They remain behind the VM-only `jit-speculation` research feature
until a canonical compiler workload demonstrates a repeatable end-to-end benefit.

The stable native path contains a deliberately narrow read-only LICM subset. Its
lazy loop-activation memoization is emitted only when verifier-bound typed facts
produce flow-sensitive ownership/alias evidence, every operand is invariant, and
the existing heap-provenance scan proves that no overlapping write occurs. The
legacy `jit-memoization-experimental` feature remains a compatibility alias; it
does not widen this production proof. OSR selection and helper-hoisting consume
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

Closure speculation remains a runnable research implementation behind
`jit-speculation`, with all-feature differential tests and dedicated scorecard
cases. It is not compiled into the ordinary SDK `native-jit` feature. Promotion
requires controlled-hardware end-to-end evidence in addition to differential
correctness; a microbenchmark-only improvement is insufficient.

Nested and loop-carried struct scalar replacement is retained for research behind
the VM-only `jit-struct-sr-experimental` feature. The ordinary SDK `native-jit`
path leaves those aggregates unchanged and fails closed to the verified
interpreter. Re-entry into the supported baseline requires canonical workload
evidence under the benchmark retention rule.

The Cranelift engine crate is not independently published. Its public root exposes
only the VM-facing engine, validated IR, typed options/outcomes, host-helper
contract, and prepared-call boundary. Raw call-frame layout, ABI offsets, helper
function aliases, codegen internals, and module implementation types remain
crate-private.

Native rewrites carry
`NativeInstructionOrigin { source_ip, resume_ip, source_cost }` in one owned
pipeline state. JIT instruction indices are CFG identities only. A pass may
temporarily return a local new-to-previous map, but only the pipeline state
composes it; source identity, interpreter resume, and accounting ownership may not
travel in unrelated parallel vectors. Expansion assigns one source cost to exactly
one generated item, fusion preserves the summed cost, and a rewrite that loses
cost is rejected before codegen. Codegen reserves explicit source cost by segment;
deopt maps expose the explicit source/resume positions to the VM.

## Native recursion

Tail recursion may be lowered to a loop. Non-tail self or group recursion uses the
host stack and therefore requires both the VM-only
`jit-recursion-experimental` feature and explicit trusted-host opt-in; it is not
available through the stable SDK. It remains experimental until implemented with
an explicit frame stack, a trampoline, or a target-backed live stack-limit check.
Static frame estimates are admission heuristics, not a hard safety proof.

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
replacement paths that enter on the canonical scorecard. Speculation and non-tail
native recursion remain research features; only the alias-gated read-only LICM
subset is retained in production. A local scorecard
run is diagnostic only; timings become a compatibility or release signal only
after a controlled-hardware baseline is checked in with machine/toolchain metadata.
