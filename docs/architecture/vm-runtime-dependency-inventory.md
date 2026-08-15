# VM runtime dependency inventory

`rsscript-vm` is the execution engine. Its normal dependency section is kept
small and reviewed so deterministic library implementations do not silently
become part of the VM trusted computing base.

## Approved runtime dependencies

| Dependency | Owner | Reason | Removal condition |
| --- | --- | --- | --- |
| `rsscript-abi-model`, `rsscript-bytecode`, `rsscript-diagnostics`, `rsscript-operation`, `rsscript-provider-api`, `rsscript-text` | Core execution boundary | Versioned ABI, verified program, diagnostics, operation controls, Provider dispatch, and text utilities. | Stable Core contracts; not scheduled for removal. |
| `rsscript-corelib` | Deterministic builtin boundary | Encoding, regex, date, hash/HMAC, gzip, YAML transcoding, and generic collection algorithms. | Remains the one-way pure-library dependency. |
| `serde` | VM model serialization | Serialization derives for the VM's verified-program and value-model support types. | Re-evaluate only if the VM model’s serialization contract changes. |
| `serde_json` | Legacy JSON value adapter | `VmValue::Json` and compatibility `NativeValue::Json` currently use `serde_json::Value`. | P06.2/P06.4 canonical `WireValue` cutover and legacy escape-variant deletion. |
| `vm-jit` (optional) | Experimental native tier | Feature-gated JIT implementation; excluded from the default Core VM closure. | Remains experimental or moves to an external lab. |

The VM must not directly add algorithm crates for encoding, regex, time/date,
compression, hashing, HMAC, or YAML. Such code belongs in `rsscript-corelib`
when it is deterministic and independent of VM values; host-visible services
remain Provider calls.

This inventory describes dependencies only. It does not make the in-process VM
a security isolation boundary; see the threat model for trust and runner
requirements.
