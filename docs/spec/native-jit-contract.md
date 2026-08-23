# Native JIT contract

The native JIT is an optional accelerator for trusted, in-process execution. It
does not add an isolation boundary and never changes Provider authority.

The supported `native-jit` feature is intentionally a bounded *feature surface*,
not yet an accelerator for bounded executions. Whole-function native entry
currently declines when step, cancellation, deadline, allocation, live-memory,
intrinsic-call, or Provider-call limits are armed. The interpreter then executes
the request with the original limits. This fail-closed behavior keeps ordinary
bounded execution correct; hosts only receive native acceleration from the
explicit trusted/unbounded execution mode until accounting parity is implemented.

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
- Every call crosses the versioned `JitCallFrame` ABI. The frame owns bail,
  safepoint, deoptimization, depth, limit, and host-context state for that call.
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
  worth implementing; they do not change execution semantics.
- ABI v3 distinguishes a planned continuation `Yield` from `Deopt`. A yield
  commits completed region work, materializes its bounded live scalar state, and
  resumes the VM at the barrier instruction. A deopt aborts transactional work
  and follows the existing precise-resume or replay contract. The initial stable
  continuation slice admits only straight-line scalar regions around non-`mut`
  `CallKnown` barriers and function returns; heap, branching, async, Provider, and
  resource barriers remain interpreter-owned until their contracts are added.
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

Loop-invariant host-helper memoization is likewise excluded from the stable SDK
path and compiled only by `jit-memoization-experimental`. Its CFG scope proof and
runtime memo state must earn retention through the same canonical scorecard.

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

Native rewrites carry `NativeInstructionOrigin { bytecode_ip, resume_ip }` in one
owned pipeline state. A pass may temporarily return a local new-to-previous map,
but only the pipeline state composes it; source and deopt-resume identity may not
travel in unrelated parallel vectors.

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

## Hardening gate

The weekly hardening workflow runs interpreter/native differential tests, forced
deoptimization and rollback cases, 64/128/256 KiB host-stack entries, guard-page
flat-buffer bounds tests, AddressSanitizer coverage for host wrappers, structured
IR fuzzing, and the workload scorecard. ASan does not instrument generated machine
code; guard pages and canary/boundary fixtures cover direct native memory accesses.

The stable retention set is baseline scalar/flat-data execution, native leaf-call
chains, transactional helpers, precise deopt, and the Option/Result/Variant scalar
replacement paths that enter on the canonical scorecard. Speculation, non-tail
native recursion, and helper memoization are research features. A local scorecard
run is diagnostic only; timings become a compatibility or release signal only
after a controlled-hardware baseline is checked in with machine/toolchain metadata.
