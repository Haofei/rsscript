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

The optional JIT artifact format is versioned independently by
`vm_jit::IR_VERSION`, currently `25`.
