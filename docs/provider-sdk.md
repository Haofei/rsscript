# Provider SDK

Providers are trusted host adapters. They implement symbols declared by `.rssi`
interfaces; they do not change whether an RSScript program is valid, and they
are not a sandbox boundary.

## Contract layers

Keep these three kinds of data separate:

- Language semantics: symbol, parameter names and structured `WireType` values,
  `read`/`mut`/`take`,
  retention, return type, and sync/async shape. These form the
  `FunctionSignature` and its deterministic `signature_hash`.
- Runtime linkage: provider identity/version, supported runtime ABI, entry
  name, blocking/cancellation behavior, thread safety, reentrancy, cleanup, and
  error mapping.
- Optional review metadata: deployment targets, data classifications, or
  organization labels. Review adapters may consume this metadata but the
  compiler and verifier do not.

`ProviderRegistry` rejects an invalid provider identity, unsupported ABI,
duplicate symbol, missing/extra implementation, or descriptor/implementation
signature mismatch. `Runtime::link` resolves every bytecode import and rejects
an unresolved symbol, ABI mismatch, or import signature mismatch. It returns an
unforgeable `LinkedArtifact` that owns the resolved call table and execution
limits; only that linked phase exposes `run` and `execute`. Provider code is not
called until this preflight succeeds.

## Implementing a provider

1. Declare the external function in a `.rssi` file.
2. Build one `ProviderFunctionDescriptor` per symbol. Fill every behavioral
   field truthfully; `MayBlock` work must not be described as non-blocking, and
   cancellation/cleanup promises are part of the provider contract.
3. Return a `BTreeMap<ExternalSymbol, ProviderFunction<...>>` containing
   exactly the declared symbols and the same semantic signatures. Use
   `WireInterpreterFn` / `AsyncWireInterpreterFn` when no parameter is `mut`;
   use `WireMutationInterpreterFn` / `AsyncWireMutationInterpreterFn` when a
   `mut` parameter needs a result plus its checked write-backs. These canonical
   callables receive and return `WireValue` directly. `NativeInterpreterFn`
   and `AsyncInterpreterFn` exist only in the explicit `compatibility` adapter
   for legacy VM/native callers.
4. Register descriptor and implementations at the host composition root.
5. Call `Runtime::link`, then execute the resulting `LinkedArtifact` with an
   `ExecutionRequest`.
6. Run `rsscript_provider_conformance::assert_provider_conforms` in the
   Provider's test suite, then add Provider-specific tests for every advertised
   cancellation, cleanup, host-context, budget, and error-mapping behavior.

The descriptor fields are:

| Field | Meaning |
| --- | --- |
| `provider_id`, `provider_version` | Stable provider identity and release |
| `supported_abi` | Compatible `RUNTIME_ABI_VERSION` values |
| `symbol` | Fully qualified external symbol used by `.rssi` and bytecode |
| `signature` | Semantic call contract; its hash is embedded in bytecode |
| `entry` | Provider-local implementation name for diagnostics/metadata |
| `call_mode` | Synchronous or asynchronous call contract |
| `blocking` | Whether the call may block its executing thread |
| `cancellation` | Not applicable, cooperative, or abort-safe |
| `thread_safe`, `reentrant` | Concurrency guarantees |
| `resource_cleanup` | Structured ownership/cleanup mode on success, error, and cancellation |
| `error_mapping` | Versioned structured host-to-RSScript error mapping |

Provider callables return `ProviderError { code, message, retryable, details }`.
Synchronous functions use `WireInterpreterFn::new_contextual` to receive a
borrowed `ProviderCallContext`; asynchronous functions use
`AsyncWireInterpreterFn::new` and receive an owned `AsyncProviderCallContext`
that is safe to retain across suspension. Both contexts carry the monotonic
deadline, cancellation token, call id, remaining byte/output budgets, host-call context,
trace sink, and VM-owned resource registry. Providers should check cancellation
around potentially blocking or long-running work; the runtime checks it before
entry and after cooperative async completion.

The synchronous dispatcher rejects `MayBlock` functions by default. A host that has
placed execution on an appropriate blocking lane must opt in with
`RunLimits::allow_blocking_provider_calls = true`. Async Provider futures are
polled by the VM task scheduler and never execute `MayBlock` work on that lane;
blocking work must be moved to a host executor before its result is awaited.
Descriptor call mode, semantic async signature, and callable kind are checked
during registration, before bytecode can execute.

Providers that create retained host resources register a `ProviderResource`
through the context and expose the returned generation-safe `ResourceHandle`.
The VM owns the table, rejects stale/reused handles, enforces `resource_limit`,
and invokes cleanup exactly once on explicit close or execution exit.
`ExecutionUsage` distinguishes `resources_cleaned` from
`resource_cleanup_failures`; a failed cleanup is never reported as successful.
It also reports peak live Provider resources and the structured-task lifecycle.
Each Provider call trace records deterministic logical request/response bytes
and elapsed time. `ExecutionTelemetry::provider_functions` aggregates those
traces by Provider identity, version, and symbol so a host can compare external
work with total execution time without parsing logs. Payload bytes describe
RSScript values only; transport framing and allocator capacity remain
Provider-specific and are intentionally excluded.

Use `rsscript-provider-api` for the safe value and descriptor types. Dynamic
library or native-plugin ABI concerns belong in a separate adapter and must not
leak into the provider contract.

## Deterministic record/replay

`replayable_wire_callable` and `replayable_async_wire_callable` are opt-in
test/debug wrappers for a Provider explicitly declared as deterministic. A
recorded tape checks the external symbol, semantic signature hash, and exact
canonical `WireValue` request sequence; replay never falls back to the real
Provider after a mismatch.

The reference tape is deliberately **in-memory only** and has no serialization
API. It accepts only the strict contract:

- deterministic replayability;
- canonical wire-value normalization;
- no value redaction (metadata-only recordings cannot be replayed);
- no declared external-state dependency; and
- in-memory retention.

Hosts that need persistence, redaction, or a model of external state must build
and audit an explicit transport on top of tape entries. Record/replay is useful
for deterministic regression tests and diagnostics; it is neither an
authorization decision nor a security proof.

## Conformance checklist

The `rsscript-provider-conformance` crate supplies the common fail-closed
preflight used by every official Provider. It checks descriptor structure,
unique entries and parameters, ABI compatibility, exact implementation
linkage, import resolution, and the runtime-owned cancellation/deadline gate.
It never performs a real Provider operation: cancelled and already-expired
contexts stop the call before Provider code receives arguments. The returned
`ProviderConformanceReport` also inventories blocking, cancellable, and
resource-producing functions for test assertions and release evidence.

This generic kit complements rather than replaces behavior tests. A Provider
that advertises cooperative cancellation, rooted host configuration, byte limits, or
resource cleanup must still demonstrate those semantics with its real call.

- Changing providers leaves `BuiltArtifact::bundle_bytes()` byte-for-byte equal.
- A changed effect, type, retention flag, result type, or async flag changes the
  signature hash and fails before execution.
- Descriptor and implementation symbol sets are identical.
- Resource cleanup holds on success, provider error, deadline, and cancellation.
- Blocking and cancellation declarations match observed behavior.
- Provider errors contain actionable context without exposing unrelated host
  state or secrets.

See `providers/fs` for a minimal concrete implementation and
`examples/embedded-report-pipeline` for memory and real-filesystem providers
running the same compiled artifact.

Host authority belongs to Provider instances and profiles, not to the Core ABI.
For example,
On Unix, `RootedFsProvider::new(root)` opens a stable root directory descriptor
and resolves every component relative to it with no-follow semantics, rejecting
traversal, symlink escapes, and canonicalize/open races without changing the
process current directory. Platforms without that race-resistant implementation
fail construction closed. This is authority narrowing, not a language
permission system.

Likewise, `HttpProvider::new(client_builder, allowed_origins)` requires the host
to name every reachable HTTP origin. It installs the allowlist on redirect hops,
applies the execution deadline with a 30-second hard ceiling, and bounds the
combined response body by the remaining runtime budget and its 16 MiB provider
ceiling.
