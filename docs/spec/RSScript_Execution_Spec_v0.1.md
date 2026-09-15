# RSScript execution specification

Compilation produces a platform-neutral executable. The compile API has no host,
deployment, or permission argument. A runner executes the same artifact with an
`ExternalFunctionRegistry` and `ExecutionControl`/limits.

`CallExternal` contains stable symbol identity, arguments, destination, and
mutation write-back positions. An unresolved symbol is a link/execution error.
Provider choice must not alter parsing, validation, HIR, lowering, or the compiled
artifact.

Execution control includes cancellation, deadline, cumulative allocation budget,
exact reachable-value live-memory limit, external-call budget, output bounds,
and recursion bounds. The live metric deduplicates shared heap nodes and subtracts
unreachable values at instruction boundaries; it excludes allocator metadata,
generated code, and Provider-owned memory, so it is deterministic across hosts.
Execution control does not include a language authority model. Resource slots are opaque and provider-owned at the
external boundary; cleanup occurs on normal return, error, cancellation, and
deadline exit.

The VM core executes arithmetic, comparison, collections, strings, control flow,
closures, type construction, resources, structured scheduler primitives, and
external calls. Operating-system behavior is supplied by explicit providers.

## Entry point

An entry point is the function named `main`. It must declare **either no
parameters, or exactly one parameter whose type is `List<String>`**, which
receives the program arguments; any other arity, or a single parameter of any
other type, is rejected before the first instruction runs
(`crates/rsscript-vm/src/reg_vm/scheduler.rs::run_program`). The return type is
unconstrained: the runner returns `main`'s value, and the execution report
carries it as a typed wire value when the declared return type parses as one and
omits it otherwise
(`crates/rsscript-vm/src/reg_vm/executable.rs::main_result_wire_value`). An
artifact with no `main` is a link error. The front end imposes no signature
constraint on `main` and accepts a file with none, so this is the runner's half
of the contract.

## Evaluation and arithmetic

- A call's arguments are evaluated left to right **as written at the call site**,
  not in parameter-declaration order: the receiver first, then every explicit
  argument in source order whatever parameter it names, then each omitted
  defaulted parameter's default expression in declaration order
  (`crates/rsscript-semantics/src/call_binding.rs`; lowering consumes the
  resulting `evaluation_index`).
- `+`, `-`, `*`, `/`, `%` on `Int` are **checked**: overflow, division by zero,
  and modulo by zero are language-level runtime errors, never wrapping and never
  a host panic (`crates/rsscript-vm/src/reg_vm/value_ops.rs`). `Int` is 64-bit
  signed. Wrapping arithmetic is opt-in through `Math.wrapping_*`.
- `Float` arithmetic is IEEE-754 and does not trap: overflow yields an infinity
  and `0.0 / 0.0` yields `NaN`. `%` on `Float` is a runtime error.

## Structured concurrency and cleanup

These rules affect program meaning and are checked independently of provider
choice or runtime limits:

- `async let` is legal only inside its lexical `task_group`.
- Every named child handle is consumed by exactly one `await`; it cannot be
  awaited before declaration, awaited twice, or referenced after group exit.
- `async let _` creates a scoped background child. The group drains it before
  the scope can return, so no child silently outlives its parent.
- `Task.cancellation_token()` requires a lexical task-group owner. Cancellation
  propagates through that owner instead of creating an unstructured global task.
- A `resource` and a live `local` value cannot cross an `await`. A local may be
  moved with `take` into the awaited operation when its signature permits it.
- A parameter marked `retains(param)` is an escape even when the call is async;
  local/resource values cannot be hidden behind that suspension boundary.
- Normal return, `?` propagation, provider error, deadline, and cancellation
  converge on the same resource-slot cleanup path. A provider-owned resource is
  released according to its declared cleanup contract.
- Cancelled channel send/receive operations do not publish a partial transfer;
  channel closure remains observable through the ordinary result contract.
- `select` tie-breaking is by source order: among the arms that have finished
  when the `select` resolves, the earliest-written arm wins, deterministically
  and independently of completion timing, task ids, and hashing
  (`crates/rsscript-vm/src/reg_vm/scheduler.rs::resolve_wait`). Source order
  only decides between arms that are already ready; it is not a priority over
  arms that are not.
- Tasks start in creation order from a FIFO ready queue and run one at a time.
  The order in which several tasks unblocked by the same event resume is **not**
  specified, and no fairness bound is promised.

Provider descriptors additionally state whether an external function is
cooperative, abort-safe, or not cancellation-aware, and whether it may block.
The runtime validates the semantic signature and ABI before executing the first
instruction; deployment metadata cannot weaken these language rules.

Conformance anchors:

| Rule group | Anchor |
| --- | --- |
| `async let` scoping and exactly-once handle consumption | `crates/rsscript-semantics/src/task_groups.rs` (module tests) |
| `await` placement, values live across an `await`, `Task.cancellation_token()` ownership | `crates/rsscript-semantics/src/await_placement.rs` (module tests) |
| resource escape from a `with` scope, and resource producers | `crates/rsscript-semantics/src/resource_flow.rs`, `crates/rsscript-semantics/src/resource_producers.rs` |
| structured task lifecycle at runtime (created / completed / cancelled / peak-live counts) | `crates/rsscript-sdk/src/tests.rs::execution_usage_reports_structured_task_lifecycle` |
| scheduler cancellation | `crates/rsscript-vm/src/reg_vm/scheduler.rs` (module tests) |
| `select` tie-breaking and the entry-point signature | `crates/rsscript-vm/src/reg_vm/scheduler.rs::resolve_wait`, `::run_program` (implementation; no dedicated module test yet) |
| checked `Int` arithmetic | `crates/rsscript-vm/src/reg_vm/value_ops.rs::eval_numeric_binary` |
| call-argument evaluation order | `crates/rsscript-semantics/src/call_binding.rs` (module tests) |
| exactly-once resource cleanup on every terminal path | `crates/rsscript-sdk/tests/execution_state_corpus.rs` |

The optional native JIT consumes an in-process, non-serialized IR and is released
in lockstep with the VM. It is not an Artifact format and carries no independent
compatibility promise. Its only FFI boundary is the explicitly versioned call
frame described in [`native-jit-contract.md`](native-jit-contract.md).
