# VM runtime dependency inventory

`rsscript-vm` is the execution engine. Its normal dependency section is kept
small and reviewed so deterministic library implementations do not silently
become part of the VM trusted computing base.

## Approved runtime dependencies

| Dependency | Owner | Reason | Removal condition |
| --- | --- | --- | --- |
| `rsscript-abi-model`, `rsscript-bytecode`, `rsscript-diagnostics`, `rsscript-operation`, `rsscript-provider-api`, `rsscript-text` | Core execution boundary | Versioned ABI, verified program, diagnostics, operation controls, Provider dispatch, and text utilities. | Stable Core contracts; not scheduled for removal. |
| `rsscript-corelib` | Deterministic builtin boundary | Encoding, regex, date, hash/HMAC, gzip, JSON/YAML structured-data representation, and generic collection algorithms. The legacy JSON adapter is re-exported only through `structured_data`, so the VM has no direct implementation-crate dependency. | Remains the one-way pure-library dependency; P06.2/P06.4 remove the legacy dynamic representation itself. |
| `serde` | VM model serialization | Serialization derives for the VM's verified-program and value-model support types. | Re-evaluate only if the VM model’s serialization contract changes. |
| `vm-jit` (optional) | Adaptive native backend | Cranelift translation, executable-memory management, OSR, and deoptimization for explicitly trusted in-process execution. It is absent from the default VM closure and cannot be selected by an Artifact. | Keep only while release workloads show a material end-to-end win and native/interpreter differential gates pass. |

The VM must not directly add algorithm crates for encoding, regex, time/date,
compression, hashing, HMAC, or YAML. Such code belongs in `rsscript-corelib`
when it is deterministic and independent of VM values; host-visible services
remain Provider calls.

The native JIT is an optional VM feature and remains outside the default Core
dependency closure. Selecting it is a host build and execution decision; source
code and Artifacts cannot request it. Bounded or isolated execution continues to
use the reference interpreter until native execution implements identical
deterministic budget accounting.

This inventory describes dependencies only. It does not make the in-process VM
a security isolation boundary; see the threat model for trust and runner
requirements.
