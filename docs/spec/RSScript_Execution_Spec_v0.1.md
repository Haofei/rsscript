# RSScript execution specification

Compilation produces a platform-neutral executable. The compile API has no host,
deployment, or permission argument. A runner executes the same artifact with an
`ExternalFunctionRegistry` and `ExecutionControl`/limits.

`CallExternal` contains stable symbol identity, arguments, destination, and
mutation write-back positions. An unresolved symbol is a link/execution error.
Provider choice must not alter parsing, validation, HIR, lowering, or the compiled
artifact.

Execution control includes cancellation, deadline, step budget, memory budget,
external-call budget, output bounds, and recursion bounds. It does not include a
language authority model. Resource slots are opaque and provider-owned at the
external boundary; cleanup occurs on normal return, error, cancellation, and
deadline exit.

The VM core executes arithmetic, comparison, collections, strings, control flow,
closures, type construction, resources, structured scheduler primitives, and
external calls. Operating-system behavior is supplied by explicit providers.

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

Provider descriptors additionally state whether an external function is
cooperative, abort-safe, or not cancellation-aware, and whether it may block.
The runtime validates the semantic signature and ABI before executing the first
instruction; deployment metadata cannot weaken these language rules.

Conformance anchors live in `tests/checker_frontend/async_resources.rs`,
`tests/vm_eval_parity/async_concurrency.rs`, and the runtime resource tests.

The optional JIT artifact format is versioned independently by
`vm_jit::IR_VERSION`, currently `25`.
