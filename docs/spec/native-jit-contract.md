# Native JIT contract

The native JIT is an optional accelerator for trusted, in-process execution. It
does not add an isolation boundary and never changes Provider authority.

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
