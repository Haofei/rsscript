# Provider SDK

Providers are trusted host adapters. They implement symbols declared by `.rssi`
interfaces; they do not change whether an RSScript program is valid, and they
are not a sandbox boundary.

## Contract layers

Keep these three kinds of data separate:

- Language semantics: symbol, parameter names/types, `read`/`mut`/`take`,
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
signature mismatch. Before execution, `Runtime` resolves every bytecode import
and rejects an unresolved symbol, ABI mismatch, or import signature mismatch.
Provider code is not called until this preflight succeeds.

## Implementing a provider

1. Declare the external function in a `.rssi` file.
2. Build one `ProviderFunctionDescriptor` per symbol. Fill every behavioral
   field truthfully; `MayBlock` work must not be described as non-blocking, and
   cancellation/cleanup promises are part of the provider contract.
3. Return a `BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>>`
   containing exactly the declared symbols and the same semantic signatures.
4. Register descriptor and implementations at the host composition root.
5. Test descriptor/implementation linking and test every advertised
   cancellation, cleanup, and error-mapping behavior.

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
| `resource_cleanup_contract` | Ownership and cleanup behavior on success, error, and cancellation |
| `error_mapping` | How host errors become RSScript errors |

Use `rsscript-provider-api` for the safe value and descriptor types. Dynamic
library or native-plugin ABI concerns belong in a separate adapter and must not
leak into the provider contract.

## Conformance checklist

- Changing providers leaves `CompiledPackage::bytecode()` byte-for-byte equal.
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
